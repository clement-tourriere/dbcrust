//! The DynamoDB requests behind each command and PartiQL `SELECT`. Every request
//! is bounded before it is sent (a `Limit` that never exceeds the rows still
//! wanted, a page budget on every loop) and raced against Ctrl-C and the query
//! timeout: dropping the SDK future aborts the HTTP request.

use crate::errors::map_error;
use crate::value::{Item, items_to_result};
use crate::{LimitedResult, Shared};
use aws_sdk_dynamodb::primitives::Blob;
use aws_sdk_dynamodb::types::{
    AttributeValue, KeySchemaElement, KeyType, ReturnConsumedCapacity, ScalarAttributeType,
    TableDescription,
};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use dbcrust_core::database::{DatabaseError, StructuredQueryResult, interrupt_flag, query_timeout};
use dbcrust_core::dynamodb_command::{
    DynamoCommand, DynamoInput, KeyCondition, KeyOp, Verb, partiql_target,
};
use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

/// How often a running request re-checks the interrupt flag and the deadline.
const POLL_INTERVAL: Duration = Duration::from_millis(100);
/// Names per ListTables page (the DynamoDB maximum).
const LIST_TABLES_PAGE: i32 = 100;
/// Items per request, however many rows are still wanted (a request also
/// stops at 1 MB of data).
const PAGE_ITEMS: usize = 1000;

type Row = Vec<Option<String>>;

fn cell(value: impl Into<String>) -> Option<String> {
    Some(value.into())
}

fn result(columns: &[&str], rows: Vec<Row>) -> StructuredQueryResult {
    StructuredQueryResult {
        columns: columns.iter().map(|c| c.to_string()).collect(),
        rows,
    }
}

/// The partition and optional sort key attribute of a table or an index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KeySchema {
    pub(crate) partition: String,
    pub(crate) sort: Option<String>,
}

impl KeySchema {
    fn from_elements(elements: &[KeySchemaElement]) -> Option<Self> {
        let find = |kind: KeyType| {
            elements
                .iter()
                .find(|element| element.key_type() == &kind)
                .map(|element| element.attribute_name().to_string())
        };
        Some(Self {
            partition: find(KeyType::Hash)?,
            sort: find(KeyType::Range),
        })
    }

    pub(crate) fn names(&self) -> Vec<String> {
        let mut names = vec![self.partition.clone()];
        names.extend(self.sort.clone());
        names
    }

    pub(crate) fn describe_for_metadata(&self) -> String {
        self.describe()
    }

    fn describe(&self) -> String {
        match &self.sort {
            Some(sort) => format!("{} (partition), {} (sort)", self.partition, sort),
            None => format!("{} (partition)", self.partition),
        }
    }
}

/// What the key-based commands need from `DescribeTable`.
#[derive(Debug, Clone)]
pub(crate) struct TableInfo {
    pub(crate) name: String,
    pub(crate) keys: KeySchema,
    /// Global and local secondary indexes by name.
    pub(crate) indexes: BTreeMap<String, KeySchema>,
    /// Scalar type (`S`, `N`, `B`) of every attribute used in a key.
    pub(crate) types: HashMap<String, char>,
}

impl TableInfo {
    fn from_description(table: &TableDescription) -> Option<Self> {
        let keys = KeySchema::from_elements(table.key_schema())?;
        let mut indexes = BTreeMap::new();
        for index in table.global_secondary_indexes() {
            if let (Some(name), Some(schema)) = (
                index.index_name(),
                KeySchema::from_elements(index.key_schema()),
            ) {
                indexes.insert(name.to_string(), schema);
            }
        }
        for index in table.local_secondary_indexes() {
            if let (Some(name), Some(schema)) = (
                index.index_name(),
                KeySchema::from_elements(index.key_schema()),
            ) {
                indexes.insert(name.to_string(), schema);
            }
        }
        let types = table
            .attribute_definitions()
            .iter()
            .map(|definition| {
                let code = match definition.attribute_type() {
                    ScalarAttributeType::N => 'N',
                    ScalarAttributeType::B => 'B',
                    _ => 'S',
                };
                (definition.attribute_name().to_string(), code)
            })
            .collect();
        Some(Self {
            name: table.table_name().unwrap_or_default().to_string(),
            keys,
            indexes,
            types,
        })
    }

    /// The key schema a read goes through: the index's, or the table's.
    fn schema_for(&self, index: Option<&str>) -> Result<&KeySchema, DatabaseError> {
        match index {
            None => Ok(&self.keys),
            Some(name) => self.indexes.get(name).ok_or_else(|| {
                let known: Vec<&str> = self.indexes.keys().map(String::as_str).collect();
                DatabaseError::QueryError(format!(
                    "table {} has no index '{name}'{}",
                    self.name,
                    if known.is_empty() {
                        " (it has no secondary index)".to_string()
                    } else {
                        format!(" (indexes: {})", known.join(", "))
                    }
                ))
            }),
        }
    }

    /// The key attribute names to show first: the table's, then the index's.
    pub(crate) fn key_columns(&self, index: Option<&str>) -> Vec<String> {
        let mut names = self.keys.names();
        if let Some(schema) = index.and_then(|index| self.indexes.get(index)) {
            for name in schema.names() {
                if !names.contains(&name) {
                    names.push(name);
                }
            }
        }
        names
    }

    /// The `AttributeValue` of a typed key from its text.
    fn key_value(&self, attribute: &str, text: &str) -> Result<AttributeValue, DatabaseError> {
        match self.types.get(attribute) {
            Some('N') => {
                let valid = !text.is_empty()
                    && text.parse::<f64>().is_ok_and(f64::is_finite)
                    && text
                        .chars()
                        .all(|c| c.is_ascii_digit() || matches!(c, '-' | '+' | '.' | 'e' | 'E'));
                if valid {
                    Ok(AttributeValue::N(text.to_string()))
                } else {
                    Err(DatabaseError::QueryError(format!(
                        "key {attribute} is a number; '{text}' is not one"
                    )))
                }
            }
            Some('B') => STANDARD
                .decode(text)
                .map(|bytes| AttributeValue::B(Blob::new(bytes)))
                .map_err(|_| {
                    DatabaseError::QueryError(format!(
                        "key {attribute} is binary: give its value as base64"
                    ))
                }),
            _ => Ok(AttributeValue::S(text.to_string())),
        }
    }
}

/// Items gathered over several requests.
#[derive(Default)]
struct Collected {
    items: Vec<Item>,
    /// Capacity units consumed, summed over the requests.
    capacity: f64,
    requests: usize,
    /// Whether a bound stopped the read before DynamoDB had no more.
    more: bool,
}

impl Collected {
    fn add_capacity(&mut self, units: Option<f64>) {
        self.capacity += units.unwrap_or(0.0);
    }
}

/// Every request after the first one is a cost the user did not type: say how
/// much the whole command consumed.
fn capacity_notice(collected: &Collected) -> String {
    format!(
        "consumed {} capacity units in {} request{} for {} item{}",
        format_units(collected.capacity),
        collected.requests,
        if collected.requests == 1 { "" } else { "s" },
        collected.items.len(),
        if collected.items.len() == 1 { "" } else { "s" },
    )
}

fn format_units(units: f64) -> String {
    if (units - units.round()).abs() < 1e-9 {
        format!("{}", units.round() as i64)
    } else {
        format!("{units:.1}")
    }
}

pub(crate) fn human_bytes(bytes: i64) -> String {
    let bytes = bytes.max(0) as f64;
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} B", bytes as i64)
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

impl Shared {
    /// Run a request future, giving up on Ctrl-C or when the query timeout
    /// passes. Dropping the future aborts the HTTP request.
    pub(crate) async fn guard<T>(
        &self,
        request: impl Future<Output = Result<T, DatabaseError>>,
    ) -> Result<T, DatabaseError> {
        let interrupt = interrupt_flag();
        let started = Instant::now();
        tokio::pin!(request);
        loop {
            tokio::select! {
                answer = &mut request => return answer,
                _ = tokio::time::sleep(POLL_INTERVAL) => {
                    if interrupt.load(Ordering::Relaxed) {
                        return Err(DatabaseError::QueryError("query interrupted".to_string()));
                    }
                    if let Some(limit) = query_timeout() {
                        if started.elapsed() >= limit {
                            return Err(DatabaseError::QueryError(format!(
                                "request timed out after {}s",
                                limit.as_secs()
                            )));
                        }
                    }
                }
            }
        }
    }

    fn table_or_current(&self, table: Option<String>) -> Result<String, DatabaseError> {
        table.or_else(|| self.table()).ok_or_else(|| {
            DatabaseError::QueryError(
                "no table selected: name one (`scan Orders`), connect to dynamodb://REGION/TABLE, \
                 or switch with \\c TABLE (`tables` lists them)"
                    .to_string(),
            )
        })
    }

    /// `DescribeTable`, cached per table for the key-based commands.
    pub(crate) async fn table_info(&self, table: &str) -> Result<Arc<TableInfo>, DatabaseError> {
        if let Some(info) = self.cached_info(table) {
            return Ok(info);
        }
        let description = self.describe(table).await?;
        let info = TableInfo::from_description(&description).ok_or_else(|| {
            DatabaseError::QueryError(format!("table {table} reported no key schema"))
        })?;
        let info = Arc::new(info);
        self.cache_info(table, info.clone());
        Ok(info)
    }

    pub(crate) async fn describe(&self, table: &str) -> Result<TableDescription, DatabaseError> {
        let secrets = self.secrets();
        let request = self.client.describe_table().table_name(table);
        let output = self
            .guard(async { request.send().await.map_err(|e| map_error(e, &secrets)) })
            .await?;
        output.table.ok_or_else(|| {
            DatabaseError::QueryError(format!("DescribeTable returned nothing for {table}"))
        })
    }

    /// Run one parsed line.
    pub(crate) async fn execute(&self, input: DynamoInput) -> Result<LimitedResult, DatabaseError> {
        match input {
            DynamoInput::PartiQl(statement) => self.partiql(&statement).await,
            DynamoInput::Command(command) => match command {
                DynamoCommand::Help => Ok(LimitedResult::new(help())),
                DynamoCommand::Tables { limit } => self.tables_result(limit).await,
                DynamoCommand::Describe { table } => {
                    let table = self.table_or_current(table)?;
                    self.describe_result(&table).await
                }
                DynamoCommand::Scan {
                    table,
                    index,
                    limit,
                } => {
                    let table = self.table_or_current(table)?;
                    self.scan(&table, index.as_deref(), limit).await
                }
                DynamoCommand::Get { table, keys } => {
                    let table = self.table_or_current(table)?;
                    self.get(&table, &keys).await
                }
                DynamoCommand::Query {
                    table,
                    index,
                    keys,
                    limit,
                    descending,
                } => {
                    let table = self.table_or_current(table)?;
                    self.query(&table, index.as_deref(), &keys, limit, descending)
                        .await
                }
            },
        }
    }

    /// The names of the account's tables, at most `limit`; with whether the
    /// listing was cut short.
    pub(crate) async fn list_tables(
        &self,
        limit: usize,
    ) -> Result<(Vec<String>, bool), DatabaseError> {
        let secrets = self.secrets();
        let mut names: Vec<String> = Vec::new();
        let mut start: Option<String> = None;
        let mut more = false;
        for page in 0..self.limits.max_pages {
            let mut request = self.client.list_tables().limit(LIST_TABLES_PAGE);
            if let Some(start) = start.take() {
                request = request.exclusive_start_table_name(start);
            }
            let output = self
                .guard(async { request.send().await.map_err(|e| map_error(e, &secrets)) })
                .await?;
            names.extend(output.table_names().iter().cloned());
            start = output.last_evaluated_table_name().map(str::to_string);
            if names.len() >= limit {
                more = start.is_some() || names.len() > limit;
                names.truncate(limit);
                break;
            }
            if start.is_none() {
                break;
            }
            if page + 1 == self.limits.max_pages {
                more = true;
            }
        }
        Ok((names, more))
    }

    async fn tables_result(&self, limit: Option<usize>) -> Result<LimitedResult, DatabaseError> {
        let wanted = limit
            .unwrap_or(self.limits.max_rows)
            .min(self.limits.max_rows);
        let (names, more) = self.list_tables(wanted).await?;
        let mut out = LimitedResult::new(result(
            &["table"],
            names.into_iter().map(|name| vec![cell(name)]).collect(),
        ));
        if more {
            out.truncated = true;
            out.warnings.push(format!(
                "only the first {wanted} tables are shown (raise the limit, or the max_rows / max_pages options)"
            ));
        }
        Ok(out)
    }

    async fn describe_result(&self, table: &str) -> Result<LimitedResult, DatabaseError> {
        let description = self.describe(table).await?;
        let mut rows: Vec<Row> = Vec::new();
        let mut add = |property: &str, value: String| {
            rows.push(vec![cell(property), cell(value)]);
        };
        add(
            "table",
            description.table_name().unwrap_or(table).to_string(),
        );
        if let Some(status) = description.table_status() {
            add("status", status.as_str().to_string());
        }
        if let Some(schema) = KeySchema::from_elements(description.key_schema()) {
            add("keys", schema.describe());
        }
        let types: Vec<String> = description
            .attribute_definitions()
            .iter()
            .map(|definition| {
                format!(
                    "{} {}",
                    definition.attribute_name(),
                    definition.attribute_type().as_str()
                )
            })
            .collect();
        if !types.is_empty() {
            add("key attribute types", types.join(", "));
        }
        let billing = description
            .billing_mode_summary()
            .and_then(|summary| summary.billing_mode())
            .map(|mode| mode.as_str().to_string());
        match (&billing, description.provisioned_throughput()) {
            (Some(mode), _) if mode == "PAY_PER_REQUEST" => add("billing", "on demand".to_string()),
            (_, Some(throughput)) => add(
                "billing",
                format!(
                    "provisioned: {} read / {} write units",
                    throughput.read_capacity_units().unwrap_or(0),
                    throughput.write_capacity_units().unwrap_or(0)
                ),
            ),
            (Some(mode), None) => add("billing", mode.to_lowercase()),
            (None, None) => {}
        }
        if let Some(count) = description.item_count() {
            add(
                "items (approximate, refreshed about every 6 hours)",
                count.to_string(),
            );
        }
        if let Some(size) = description.table_size_bytes() {
            add("size (approximate)", human_bytes(size));
        }
        for index in description.global_secondary_indexes() {
            if let Some(schema) = KeySchema::from_elements(index.key_schema()) {
                let projection = index
                    .projection()
                    .and_then(|projection| projection.projection_type())
                    .map(|kind| kind.as_str().to_string())
                    .unwrap_or_default();
                let status = index
                    .index_status()
                    .map(|status| status.as_str().to_string())
                    .unwrap_or_default();
                add(
                    &format!("global index {}", index.index_name().unwrap_or_default()),
                    format!("{}; projects {projection}; {status}", schema.describe()),
                );
            }
        }
        for index in description.local_secondary_indexes() {
            if let Some(schema) = KeySchema::from_elements(index.key_schema()) {
                let projection = index
                    .projection()
                    .and_then(|projection| projection.projection_type())
                    .map(|kind| kind.as_str().to_string())
                    .unwrap_or_default();
                add(
                    &format!("local index {}", index.index_name().unwrap_or_default()),
                    format!("{}; projects {projection}", schema.describe()),
                );
            }
        }
        if let Some(stream) = description.stream_specification() {
            if stream.stream_enabled() {
                add(
                    "stream",
                    stream
                        .stream_view_type()
                        .map(|kind| kind.as_str().to_string())
                        .unwrap_or_else(|| "enabled".to_string()),
                );
            }
        }
        if let Some(protected) = description.deletion_protection_enabled() {
            add("deletion protection", protected.to_string());
        }
        if let Some(class) = description
            .table_class_summary()
            .and_then(|summary| summary.table_class())
        {
            add("table class", class.as_str().to_string());
        }
        if let Some(arn) = description.table_arn() {
            add("arn", arn.to_string());
        }
        Ok(LimitedResult::new(result(&["property", "value"], rows)))
    }

    async fn scan(
        &self,
        table: &str,
        index: Option<&str>,
        limit: Option<usize>,
    ) -> Result<LimitedResult, DatabaseError> {
        let secrets = self.secrets();
        let wanted = limit
            .unwrap_or(self.limits.max_rows)
            .min(self.limits.max_rows);
        let mut collected = Collected::default();
        let mut start: Option<Item> = None;
        loop {
            let page = (wanted - collected.items.len()).clamp(1, PAGE_ITEMS);
            let mut request = self
                .client
                .scan()
                .table_name(table)
                .limit(page as i32)
                .return_consumed_capacity(ReturnConsumedCapacity::Total);
            if let Some(index) = index {
                request = request.index_name(index);
            }
            if self.limits.consistent_read && index.is_none() {
                request = request.consistent_read(true);
            }
            if let Some(start) = start.take() {
                request = request.set_exclusive_start_key(Some(start));
            }
            let output = self
                .guard(async { request.send().await.map_err(|e| map_error(e, &secrets)) })
                .await?;
            collected.requests += 1;
            collected.add_capacity(output.consumed_capacity().and_then(|c| c.capacity_units()));
            collected.items.extend(output.items.unwrap_or_default());
            start = output.last_evaluated_key;
            if start.is_none() {
                break;
            }
            if collected.items.len() >= wanted || collected.requests >= self.limits.max_pages {
                collected.more = true;
                break;
            }
        }
        collected.items.truncate(wanted);
        let keys = self
            .table_info(table)
            .await
            .map(|info| info.key_columns(index))
            .unwrap_or_default();
        let mut out = LimitedResult::new(items_to_result(&collected.items, &keys));
        out.warnings.push(format!(
            "scan reads items without a key condition and is billed for everything it reads; \
             `query` is the cheap read{}",
            if limit.is_none() {
                format!(" (this scan is bounded to {wanted} items; add --limit to choose)")
            } else {
                String::new()
            }
        ));
        out.notices.push(capacity_notice(&collected));
        if collected.more {
            out.truncated = true;
            out.warnings.push(format!(
                "stopped after {} items: the table has more (raise --limit, or max_rows / max_pages)",
                collected.items.len()
            ));
        }
        Ok(out)
    }

    /// The key a `get` or `query` addresses, checked against the key schema.
    fn conditions_for(
        info: &TableInfo,
        schema: &KeySchema,
        index: Option<&str>,
        conditions: &[KeyCondition],
        sort_prefix_allowed: bool,
    ) -> Result<(AttributeValue, Option<(AttributeValue, KeyOp)>), DatabaseError> {
        let what = match index {
            Some(index) => format!("index {index} of {}", info.name),
            None => format!("table {}", info.name),
        };
        let describe = schema.describe();
        let mut partition = None;
        let mut sort = None;
        for condition in conditions {
            if condition.name == schema.partition {
                if condition.op != KeyOp::Eq {
                    return Err(DatabaseError::QueryError(format!(
                        "the partition key {} only supports =",
                        condition.name
                    )));
                }
                if partition
                    .replace(info.key_value(&condition.name, &condition.value)?)
                    .is_some()
                {
                    return Err(DatabaseError::QueryError(format!(
                        "{} is given twice",
                        condition.name
                    )));
                }
            } else if schema.sort.as_deref() == Some(condition.name.as_str()) {
                if condition.op == KeyOp::BeginsWith && !sort_prefix_allowed {
                    return Err(DatabaseError::QueryError(
                        "get reads one item: use NAME=VALUE".to_string(),
                    ));
                }
                if condition.op == KeyOp::BeginsWith
                    && info.types.get(&condition.name) == Some(&'N')
                {
                    return Err(DatabaseError::QueryError(format!(
                        "{} is a number: ^= (begins_with) needs a string or binary sort key",
                        condition.name
                    )));
                }
                if sort
                    .replace((
                        info.key_value(&condition.name, &condition.value)?,
                        condition.op,
                    ))
                    .is_some()
                {
                    return Err(DatabaseError::QueryError(format!(
                        "{} is given twice",
                        condition.name
                    )));
                }
            } else {
                return Err(DatabaseError::QueryError(format!(
                    "{} is not a key of {what} (keys: {describe})",
                    condition.name
                )));
            }
        }
        let Some(partition) = partition else {
            return Err(DatabaseError::QueryError(format!(
                "give the partition key {} of {what}: {}=VALUE",
                schema.partition, schema.partition
            )));
        };
        Ok((partition, sort))
    }

    async fn get(
        &self,
        table: &str,
        conditions: &[KeyCondition],
    ) -> Result<LimitedResult, DatabaseError> {
        let secrets = self.secrets();
        let info = self.table_info(table).await?;
        let (partition, sort) = Self::conditions_for(&info, &info.keys, None, conditions, false)?;
        let mut key: Item = HashMap::new();
        key.insert(info.keys.partition.clone(), partition);
        match (&info.keys.sort, sort) {
            (Some(name), Some((value, _))) => {
                key.insert(name.clone(), value);
            }
            (Some(name), None) => {
                return Err(DatabaseError::QueryError(format!(
                    "table {table} also has the sort key {name}: give {name}=VALUE (or `query` for \
                     all of a partition)"
                )));
            }
            (None, _) => {}
        }
        let mut request = self
            .client
            .get_item()
            .table_name(table)
            .set_key(Some(key))
            .return_consumed_capacity(ReturnConsumedCapacity::Total);
        if self.limits.consistent_read {
            request = request.consistent_read(true);
        }
        let output = self
            .guard(async { request.send().await.map_err(|e| map_error(e, &secrets)) })
            .await?;
        let mut collected = Collected {
            requests: 1,
            ..Default::default()
        };
        collected.add_capacity(output.consumed_capacity().and_then(|c| c.capacity_units()));
        collected.items.extend(output.item);
        let mut out =
            LimitedResult::new(items_to_result(&collected.items, &info.key_columns(None)));
        if collected.items.is_empty() {
            out.result.columns = info.key_columns(None);
            out.notices.push("no item has that key".to_string());
        }
        out.notices.push(capacity_notice(&collected));
        Ok(out)
    }

    async fn query(
        &self,
        table: &str,
        index: Option<&str>,
        conditions: &[KeyCondition],
        limit: Option<usize>,
        descending: bool,
    ) -> Result<LimitedResult, DatabaseError> {
        let secrets = self.secrets();
        let info = self.table_info(table).await?;
        let schema = info.schema_for(index)?;
        let (partition, sort) = Self::conditions_for(&info, schema, index, conditions, true)?;

        let mut expression = "#p = :p".to_string();
        let mut names = HashMap::from([("#p".to_string(), schema.partition.clone())]);
        let mut values = HashMap::from([(":p".to_string(), partition)]);
        if let (Some(name), Some((value, op))) = (&schema.sort, sort) {
            expression.push_str(match op {
                KeyOp::Eq => " AND #s = :s",
                KeyOp::BeginsWith => " AND begins_with(#s, :s)",
            });
            names.insert("#s".to_string(), name.clone());
            values.insert(":s".to_string(), value);
        }

        let wanted = limit
            .unwrap_or(self.limits.max_rows)
            .min(self.limits.max_rows);
        let mut collected = Collected::default();
        let mut start: Option<Item> = None;
        loop {
            let page = (wanted - collected.items.len()).clamp(1, PAGE_ITEMS);
            let mut request = self
                .client
                .query()
                .table_name(table)
                .key_condition_expression(&expression)
                .set_expression_attribute_names(Some(names.clone()))
                .set_expression_attribute_values(Some(values.clone()))
                .limit(page as i32)
                .scan_index_forward(!descending)
                .return_consumed_capacity(ReturnConsumedCapacity::Total);
            if let Some(index) = index {
                request = request.index_name(index);
            }
            if self.limits.consistent_read && index.is_none() {
                request = request.consistent_read(true);
            }
            if let Some(start) = start.take() {
                request = request.set_exclusive_start_key(Some(start));
            }
            let output = self
                .guard(async { request.send().await.map_err(|e| map_error(e, &secrets)) })
                .await?;
            collected.requests += 1;
            collected.add_capacity(output.consumed_capacity().and_then(|c| c.capacity_units()));
            collected.items.extend(output.items.unwrap_or_default());
            start = output.last_evaluated_key;
            if start.is_none() {
                break;
            }
            if collected.items.len() >= wanted || collected.requests >= self.limits.max_pages {
                collected.more = true;
                break;
            }
        }
        collected.items.truncate(wanted);
        let mut out =
            LimitedResult::new(items_to_result(&collected.items, &info.key_columns(index)));
        out.notices.push(capacity_notice(&collected));
        if collected.more {
            out.truncated = true;
            out.warnings.push(format!(
                "stopped after {} items: the partition has more (raise --limit, or max_rows / max_pages)",
                collected.items.len()
            ));
        }
        Ok(out)
    }

    async fn partiql(&self, statement: &str) -> Result<LimitedResult, DatabaseError> {
        let secrets = self.secrets();
        let wanted = self.limits.max_rows;
        let mut collected = Collected::default();
        let mut token: Option<String> = None;
        loop {
            let page = (wanted - collected.items.len()).clamp(1, PAGE_ITEMS);
            let mut request = self
                .client
                .execute_statement()
                .statement(statement)
                .limit(page as i32)
                .return_consumed_capacity(ReturnConsumedCapacity::Total);
            if self.limits.consistent_read {
                request = request.consistent_read(true);
            }
            if let Some(token) = token.take() {
                request = request.next_token(token);
            }
            let output = self
                .guard(async { request.send().await.map_err(|e| map_error(e, &secrets)) })
                .await?;
            collected.requests += 1;
            collected.add_capacity(output.consumed_capacity().and_then(|c| c.capacity_units()));
            collected.items.extend(output.items.unwrap_or_default());
            token = output.next_token;
            if token.is_none() {
                break;
            }
            if collected.items.len() >= wanted || collected.requests >= self.limits.max_pages {
                collected.more = true;
                break;
            }
        }
        collected.items.truncate(wanted);
        // The table's key attributes lead the columns when DescribeTable allows
        let keys = match partiql_target(statement) {
            Some((table, index)) => self
                .table_info(&table)
                .await
                .map(|info| info.key_columns(index.as_deref()))
                .unwrap_or_default(),
            None => Vec::new(),
        };
        let mut out = LimitedResult::new(items_to_result(&collected.items, &keys));
        out.notices.push(capacity_notice(&collected));
        if collected.more {
            out.truncated = true;
            out.warnings.push(format!(
                "stopped after {} items: the statement has more results (narrow the WHERE on a \
                 key, or raise the max_rows / max_pages options)",
                collected.items.len()
            ));
        }
        Ok(out)
    }
}

fn help() -> StructuredQueryResult {
    let mut rows: Vec<Row> = dbcrust_core::dynamodb_command::verbs()
        .map(|verb: Verb| vec![cell(verb.usage()), cell(verb.description())])
        .collect();
    rows.push(vec![
        cell("SELECT … FROM \"Table\" [WHERE …]"),
        cell("PartiQL read (ExecuteStatement); without a key condition it scans, billed per item read"),
    ]);
    result(&["command", "description"], rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    fn info() -> TableInfo {
        TableInfo {
            name: "Orders".to_string(),
            keys: KeySchema {
                partition: "customer".to_string(),
                sort: Some("ts".to_string()),
            },
            indexes: BTreeMap::from([(
                "by_status".to_string(),
                KeySchema {
                    partition: "status".to_string(),
                    sort: None,
                },
            )]),
            types: HashMap::from([
                ("customer".to_string(), 'S'),
                ("ts".to_string(), 'N'),
                ("status".to_string(), 'S'),
                ("blob".to_string(), 'B'),
            ]),
        }
    }

    #[rstest]
    #[case("ts", "42", Some(AttributeValue::N("42".to_string())))]
    #[case("ts", "-1.5e3", Some(AttributeValue::N("-1.5e3".to_string())))]
    #[case("ts", "abc", None)]
    #[case("ts", "NaN", None)]
    #[case("ts", "", None)]
    #[case("customer", "42", Some(AttributeValue::S("42".to_string())))]
    #[case("unknown", "x", Some(AttributeValue::S("x".to_string())))]
    #[case("blob", "AP8=", Some(AttributeValue::B(Blob::new(vec![0u8, 255]))))]
    #[case("blob", "not base64!", None)]
    fn key_values_follow_the_attribute_type(
        #[case] attribute: &str,
        #[case] text: &str,
        #[case] expected: Option<AttributeValue>,
    ) {
        assert_eq!(info().key_value(attribute, text).ok(), expected);
    }

    #[test]
    fn key_columns_lead_with_the_table_then_the_index_keys() {
        assert_eq!(info().key_columns(None), ["customer", "ts"]);
        assert_eq!(
            info().key_columns(Some("by_status")),
            ["customer", "ts", "status"]
        );
        assert_eq!(info().key_columns(Some("missing")), ["customer", "ts"]);
    }

    #[test]
    fn an_unknown_index_names_the_known_ones() {
        let error = info().schema_for(Some("nope")).unwrap_err().to_string();
        assert!(
            error.contains("nope") && error.contains("by_status"),
            "{error}"
        );
    }

    fn condition(name: &str, op: KeyOp, value: &str) -> KeyCondition {
        KeyCondition {
            name: name.to_string(),
            op,
            value: value.to_string(),
        }
    }

    #[test]
    fn conditions_are_checked_against_the_key_schema() {
        let info = info();
        let ok = Shared::conditions_for(
            &info,
            &info.keys,
            None,
            &[
                condition("ts", KeyOp::Eq, "17"),
                condition("customer", KeyOp::Eq, "c1"),
            ],
            true,
        )
        .unwrap();
        assert_eq!(ok.0, AttributeValue::S("c1".to_string()));
        assert!(matches!(ok.1, Some((AttributeValue::N(_), KeyOp::Eq))));

        let reject = |conditions: &[KeyCondition], prefix_ok: bool| {
            Shared::conditions_for(&info, &info.keys, None, conditions, prefix_ok)
                .unwrap_err()
                .to_string()
        };
        assert!(reject(&[condition("other", KeyOp::Eq, "1")], true).contains("not a key"));
        assert!(
            reject(
                &[
                    condition("customer", KeyOp::Eq, "a"),
                    condition("ts", KeyOp::BeginsWith, "1")
                ],
                true
            )
            .contains("ts is a number")
        );
        assert!(
            reject(&[condition("ts", KeyOp::Eq, "1")], true).contains("partition key customer")
        );
        assert!(
            reject(
                &[
                    condition("customer", KeyOp::Eq, "a"),
                    condition("customer", KeyOp::Eq, "b")
                ],
                true
            )
            .contains("twice")
        );
        assert!(
            reject(&[condition("customer", KeyOp::BeginsWith, "a")], true)
                .contains("only supports =")
        );
        assert!(
            reject(
                &[
                    condition("customer", KeyOp::Eq, "a"),
                    condition("ts", KeyOp::BeginsWith, "1")
                ],
                false
            )
            .contains("get reads one item")
        );
    }

    #[rstest]
    #[case(0.0, "0")]
    #[case(1.0, "1")]
    #[case(0.5, "0.5")]
    #[case(12.25, "12.2")]
    fn capacity_units_print_short(#[case] units: f64, #[case] expected: &str) {
        assert_eq!(format_units(units), expected);
    }

    #[rstest]
    #[case(0, "0 B")]
    #[case(1023, "1023 B")]
    #[case(2048, "2.0 KiB")]
    #[case(5 * 1024 * 1024, "5.0 MiB")]
    fn sizes_print_human(#[case] bytes: i64, #[case] expected: &str) {
        assert_eq!(human_bytes(bytes), expected);
    }
}
