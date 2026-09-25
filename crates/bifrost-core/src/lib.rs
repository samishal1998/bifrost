//! Bifröst core: models, validation, policy, registry and the pure reconciler. No I/O.

pub mod api;
pub mod events;
pub mod fake;
pub mod model;
pub mod policy;
pub mod reconcile;
pub mod registry;
pub mod validate;
pub use model::*;
pub use validate::{Host, Invalid, MachineId, MountId, Name, RemotePath, User};

/// Dyn-compatible async without async-trait: impls write `Box::pin(async move { .. })`.
// ponytail: BoxFuture alias instead of async-trait, PRD §5 `ctx` params dropped, inspect folds errors into Degraded; add DiscoveryContext/DriverContext when a plugin needs runtime context
pub type BoxFuture<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

/// PRD §5 minus `ctx` (nothing to carry).
/// Ok  = complete current view; invalid records are skipped with tracing::warn!, never Err.
/// Err = "could not look": the registry freezes this provider's observations until its next Ok.
pub trait DiscoveryProvider: Send + Sync {
    fn name(&self) -> &str;
    fn discover(&self) -> BoxFuture<'_, Result<Vec<MachineObservation>, DiscoveryError>>;
}

/// PRD §5 minus `ctx`. Contract:
/// - mount:   Ok only once the target is in the OS mount table. Err ⇒ nothing mounted, no process left.
/// - inspect: returns within ~5 s even on a hung FUSE mount (errors fold into Degraded).
/// - unmount: idempotent (path absent from mount table afterwards ⇒ Ok).
///   force=false never detaches a busy mount; force=true never kills a process.
pub trait MountDriver: Send + Sync {
    fn name(&self) -> &str;
    fn probe(&self) -> BoxFuture<'_, DriverAvailability>;
    fn mount(&self, req: MountRequest) -> BoxFuture<'_, Result<MountHandle, MountError>>;
    fn inspect<'a>(&'a self, h: &'a MountHandle) -> BoxFuture<'a, MountState>;
    fn unmount<'a>(
        &'a self,
        h: &'a MountHandle,
        force: bool,
    ) -> BoxFuture<'a, Result<(), MountError>>;
}
