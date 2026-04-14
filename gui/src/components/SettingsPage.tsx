import { useState, useEffect, useCallback } from "react";
import { Settings, Loader2, Check } from "lucide-react";
import * as cmd from "../commands";
import type { AppConfig } from "../types";

export function SettingsPage() {
  const [config, setConfig] = useState<AppConfig | null>(null);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState<string | null>(null);
  const [saved, setSaved] = useState<string | null>(null);

  useEffect(() => {
    cmd
      .getConfig()
      .then((c) => {
        setConfig(c);
        setLoading(false);
      })
      .catch(() => setLoading(false));
  }, []);

  const updateSetting = useCallback(
    async (key: string, value: string) => {
      setSaving(key);
      try {
        await cmd.updateConfig(key, value);
        const updated = await cmd.getConfig();
        setConfig(updated);
        setSaved(key);
        setTimeout(() => setSaved(null), 1500);
      } catch (e) {
        window.alert(`Failed to update: ${String(e)}`);
      }
      setSaving(null);
    },
    [],
  );

  if (loading) {
    return (
      <div className="h-full flex items-center justify-center bg-surface-300">
        <Loader2 className="w-5 h-5 animate-spin text-zinc-500" />
      </div>
    );
  }

  if (!config) {
    return (
      <div className="h-full flex items-center justify-center bg-surface-300 text-zinc-500">
        Failed to load configuration
      </div>
    );
  }

  interface SettingItem {
    key: string;
    label: string;
    description: string;
    type: "number" | "boolean";
    value: number | boolean;
    updatable: boolean;
  }

  const settings: { section: string; items: SettingItem[] }[] = [
    {
      section: "Query",
      items: [
        {
          key: "default_limit",
          label: "Default Row Limit",
          description:
            "Maximum number of rows returned by default when no LIMIT is specified.",
          type: "number",
          value: config.default_limit,
          updatable: true,
        },
        {
          key: "query_timeout_seconds",
          label: "Query Timeout (seconds)",
          description:
            "Maximum time allowed for a query to execute before being cancelled.",
          type: "number",
          value: config.query_timeout_seconds,
          updatable: true,
        },
        {
          key: "explain_mode",
          label: "Explain Mode",
          description:
            "When enabled, automatically prepend EXPLAIN to all queries.",
          type: "boolean",
          value: config.explain_mode,
          updatable: false,
        },
      ],
    },
    {
      section: "Display",
      items: [
        {
          key: "expanded_display",
          label: "Expanded Display",
          description:
            "Show results in expanded vertical format instead of table grid.",
          type: "boolean",
          value: config.expanded_display,
          updatable: true,
        },
        {
          key: "autocomplete_enabled",
          label: "Autocomplete",
          description:
            "Enable SQL autocomplete suggestions while typing (CLI mode).",
          type: "boolean",
          value: config.autocomplete_enabled,
          updatable: false,
        },
      ],
    },
    {
      section: "CLI Options",
      items: [
        {
          key: "show_banner",
          label: "Show Banner",
          description:
            "Display the DBCrust banner on startup in CLI mode.",
          type: "boolean",
          value: config.show_banner,
          updatable: false,
        },
        {
          key: "show_server_info",
          label: "Show Server Info",
          description:
            "Display server information after connecting in CLI mode.",
          type: "boolean",
          value: config.show_server_info,
          updatable: false,
        },
        {
          key: "pager_enabled",
          label: "Pager",
          description:
            "Use a pager (like less) for long output in CLI mode.",
          type: "boolean",
          value: config.pager_enabled,
          updatable: false,
        },
      ],
    },
  ];

  return (
    <div className="h-full overflow-auto bg-surface-300">
      <div className="max-w-2xl mx-auto p-8 animate-fade-in">
        {/* Header */}
        <div className="mb-8">
          <h1 className="text-2xl font-bold text-zinc-100 flex items-center gap-3">
            <div className="w-10 h-10 rounded-xl bg-zinc-800 flex items-center justify-center">
              <Settings className="w-5 h-5 text-zinc-400" />
            </div>
            Settings
          </h1>
          <p className="text-sm text-zinc-500 mt-2 ml-[52px]">
            Configure DBCrust preferences. Changes are saved automatically.
          </p>
        </div>

        {/* Setting Sections */}
        <div className="space-y-6">
          {settings.map(({ section, items }) => (
            <div key={section}>
              <h2 className="text-xs font-semibold text-zinc-500 uppercase tracking-wider mb-3">
                {section}
              </h2>
              <div className="bg-surface rounded-xl border border-zinc-800 divide-y divide-zinc-800/50">
                {items.map((setting) => (
                  <div
                    key={setting.key}
                    className="px-5 py-4 flex items-center justify-between gap-4"
                  >
                    <div className="min-w-0">
                      <div className="text-sm text-zinc-200 font-medium">
                        {setting.label}
                      </div>
                      <div className="text-xs text-zinc-500 mt-0.5">
                        {setting.description}
                      </div>
                    </div>
                    <div className="flex items-center gap-2 flex-shrink-0">
                      {setting.type === "boolean" ? (
                        <button
                          onClick={() => {
                            if (setting.updatable)
                              updateSetting(
                                setting.key,
                                String(!setting.value),
                              );
                          }}
                          disabled={!setting.updatable || saving === setting.key}
                          className={`relative w-10 h-5 rounded-full transition-all
                            ${!setting.updatable ? "opacity-50 cursor-not-allowed" : "cursor-pointer"}
                            ${setting.value ? "bg-accent" : "bg-zinc-700"}`}
                        >
                          <span
                            className={`absolute top-0.5 w-4 h-4 rounded-full bg-white shadow transition-all
                              ${setting.value ? "left-[22px]" : "left-0.5"}`}
                          />
                        </button>
                      ) : (
                        <div className="flex items-center gap-1.5">
                          <input
                            type="number"
                            defaultValue={setting.value as number}
                            disabled={!setting.updatable || saving === setting.key}
                            onBlur={(e) => {
                              if (
                                setting.updatable &&
                                e.target.value !== String(setting.value)
                              )
                                updateSetting(setting.key, e.target.value);
                            }}
                            onKeyDown={(e) => {
                              if (e.key === "Enter") {
                                (e.target as HTMLInputElement).blur();
                              }
                            }}
                            className="w-20 bg-surface-300 border border-zinc-700 rounded-md px-2.5 py-1
                              text-sm text-zinc-200 text-right font-mono
                              focus:outline-none focus:border-accent transition-colors
                              disabled:opacity-50"
                          />
                        </div>
                      )}
                      {saved === setting.key && (
                        <Check className="w-4 h-4 text-emerald-500" />
                      )}
                      {saving === setting.key && (
                        <Loader2 className="w-4 h-4 animate-spin text-zinc-500" />
                      )}
                    </div>
                  </div>
                ))}
              </div>
            </div>
          ))}
        </div>

        {/* Footer */}
        <p className="mt-8 text-xxs text-zinc-700 text-center">
          Configuration file: ~/.config/dbcrust/config.toml
        </p>
      </div>
    </div>
  );
}
