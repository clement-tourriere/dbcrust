//! Docker integration for DBCrust (`docker://container/db`): which daemon to
//! talk to, container discovery, reachability probing and the `docker exec`
//! relay for containers the host cannot route to.
//!
//! The crate knows nothing about database sessions: [`docker::DockerClient`]
//! returns a connected route once the caller's handshake closure succeeded on
//! it. `DockerTarget` (the pure URL form) lives in `dbcrust-core`.

pub mod docker;
pub mod docker_endpoint; // which Docker daemon: DOCKER_CONTEXT / DOCKER_HOST / contexts
pub mod docker_route; // reachability candidates, probing, route reporting
pub mod docker_tunnel; // docker exec relay for containers the host can't route to
