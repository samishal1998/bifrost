//! Test fakes: always compiled, std only (contract §2, B12). Fakes never pend, so `block_on` drives them.
#![doc(hidden)]

use crate::model::{
    DiscoveryError, DriverAvailability, MachineObservation, Metadata, MountError, MountHandle,
    MountHints, MountRequest, MountState, OnExit,
};
use crate::validate::{Host, MountId, Name};
use crate::{BoxFuture, DiscoveryProvider, MountDriver};
use std::collections::{BTreeMap, BTreeSet};
use std::future::{Future, ready};
use std::path::PathBuf;
use std::sync::Mutex;
use std::task::{Context, Poll, Waker};

pub struct FakeDiscovery {
    pub name: String,
    pub result: Mutex<Result<Vec<MachineObservation>, DiscoveryError>>,
}

impl DiscoveryProvider for FakeDiscovery {
    fn name(&self) -> &str {
        &self.name
    }
    fn discover(&self) -> BoxFuture<'_, Result<Vec<MachineObservation>, DiscoveryError>> {
        let r = self.result.lock().unwrap().clone();
        Box::pin(ready(r))
    }
}

/// `mount` and a successful `unmount` clear `states[id]`, so a set state lasts until the next mount cycle.
pub struct FakeDriver {
    pub name: String,
    pub mounted: Mutex<BTreeMap<MountId, MountHandle>>,
    pub states: Mutex<BTreeMap<MountId, MountState>>,
    pub fail_next: Mutex<BTreeSet<MountId>>,
    /// the next mount of these panics (after recording the call)
    pub panic_next: Mutex<BTreeSet<MountId>>,
    /// "mount <id>" | "unmount <id> force=<bool>"
    pub calls: Mutex<Vec<String>>,
    /// the on_exit of each successful mount (B12)
    pub exits: Mutex<BTreeMap<MountId, OnExit>>,
    /// graceful unmount of these → Err(Busy) (B12)
    pub busy: Mutex<BTreeSet<MountId>>,
}

fn mount_id(id: &str) -> MountId {
    Name::parse(id).expect("fake: invalid mount id")
}

impl FakeDriver {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.into(),
            mounted: Mutex::default(),
            states: Mutex::default(),
            fail_next: Mutex::default(),
            panic_next: Mutex::default(),
            calls: Mutex::default(),
            exits: Mutex::default(),
            busy: Mutex::default(),
        }
    }
    pub fn set_state(&self, id: &str, s: MountState) {
        self.states.lock().unwrap().insert(mount_id(id), s);
    }
    /// The next mount of `id` fails once.
    pub fn fail_next(&self, id: &str) {
        self.fail_next.lock().unwrap().insert(mount_id(id));
    }
    /// The next mount of `id` panics once, synchronously in `mount()` (no lock held).
    pub fn panic_next(&self, id: &str) {
        self.panic_next.lock().unwrap().insert(mount_id(id));
    }
    /// Removes and calls that mount's on_exit(detail) (B12). The lock is released before the callback runs.
    pub fn exit(&self, id: &str, detail: &str) {
        let f = self.exits.lock().unwrap().remove(&mount_id(id));
        if let Some(f) = f {
            f(detail.to_string());
        }
    }
}

impl MountDriver for FakeDriver {
    fn name(&self) -> &str {
        &self.name
    }
    fn probe(&self) -> BoxFuture<'_, DriverAvailability> {
        Box::pin(ready(DriverAvailability::Available {
            binary: PathBuf::from("/fake").join(&self.name),
            detail: "fake".into(),
        }))
    }
    fn mount(&self, req: MountRequest) -> BoxFuture<'_, Result<MountHandle, MountError>> {
        let id = req.spec.id.clone();
        self.calls
            .lock()
            .unwrap()
            .push(format!("mount {}", id.as_str()));
        if self.panic_next.lock().unwrap().remove(&id) {
            panic!("fake: panic_next");
        }
        let r = if self.fail_next.lock().unwrap().remove(&id) {
            Err(MountError::Failed("fake: fail_next".into()))
        } else {
            let h = MountHandle {
                id: id.clone(),
                driver: self.name.clone(),
                local_path: req.spec.local_path.clone(),
                fingerprint: req.spec.fingerprint(),
                pid: None,
            };
            self.states.lock().unwrap().remove(&id);
            self.mounted.lock().unwrap().insert(id.clone(), h.clone());
            self.exits.lock().unwrap().insert(id, req.on_exit);
            Ok(h)
        };
        Box::pin(ready(r))
    }
    fn inspect<'a>(&'a self, h: &'a MountHandle) -> BoxFuture<'a, MountState> {
        let set = self.states.lock().unwrap().get(&h.id).cloned();
        let s = set.unwrap_or_else(|| {
            if self.mounted.lock().unwrap().contains_key(&h.id) {
                MountState::Healthy
            } else {
                MountState::Missing
            }
        });
        Box::pin(ready(s))
    }
    fn unmount<'a>(
        &'a self,
        h: &'a MountHandle,
        force: bool,
    ) -> BoxFuture<'a, Result<(), MountError>> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("unmount {} force={force}", h.id.as_str()));
        let r = if !force && self.busy.lock().unwrap().contains(&h.id) {
            Err(MountError::Busy)
        } else {
            self.mounted.lock().unwrap().remove(&h.id);
            self.states.lock().unwrap().remove(&h.id);
            Ok(())
        };
        Box::pin(ready(r))
    }
}

pub fn obs(id: &str, addr: &str) -> MachineObservation {
    MachineObservation {
        id: Name::parse(id).expect("fake: invalid machine id"),
        name: id.into(),
        native_id: None,
        addresses: vec![Host::parse(addr).expect("fake: invalid address")],
        port: None,
        online: None,
        metadata: Metadata::default(),
        hints: MountHints::default(),
        ttl: None,
    }
}

/// One poll with `Waker::noop()`; fakes never pend.
pub fn block_on<F: Future>(f: F) -> F::Output {
    match std::pin::pin!(f).poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(v) => v,
        Poll::Pending => panic!("block_on: future pended (fakes never pend)"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{DriverSelector, MountSpec};
    use crate::validate::RemotePath;
    use std::sync::Arc;

    fn req(id: &str, on_exit: OnExit) -> MountRequest {
        MountRequest {
            spec: MountSpec {
                id: mount_id(id),
                machine: mount_id(id),
                host: Host::parse("127.0.0.1").unwrap(),
                port: None,
                user: None,
                remote: RemotePath::parse("~").unwrap(),
                local_path: PathBuf::from("/r").join(id),
                driver: DriverSelector::Auto,
                read_only: false,
            },
            log_path: PathBuf::new(),
            on_exit,
        }
    }

    #[test]
    fn fake_driver_fail_next_state_busy_exit() {
        let d = FakeDriver::new("fake");
        d.fail_next("a");
        assert!(block_on(d.mount(req("a", Box::new(|_| {})))).is_err());

        let exited = Arc::new(Mutex::new(None));
        let e = exited.clone();
        let on_exit: OnExit = Box::new(move |s| *e.lock().unwrap() = Some(s));
        let h = block_on(d.mount(req("a", on_exit))).unwrap();
        assert_eq!(block_on(d.inspect(&h)), MountState::Healthy);
        d.set_state("a", MountState::Stale("x".into()));
        assert_eq!(block_on(d.inspect(&h)), MountState::Stale("x".into()));

        d.busy.lock().unwrap().insert(h.id.clone());
        assert_eq!(block_on(d.unmount(&h, false)), Err(MountError::Busy));
        d.exit("a", "killed");
        assert_eq!(exited.lock().unwrap().as_deref(), Some("killed"));
        assert_eq!(block_on(d.unmount(&h, true)), Ok(()));
        assert_eq!(block_on(d.inspect(&h)), MountState::Missing);
        assert_eq!(
            *d.calls.lock().unwrap(),
            [
                "mount a",
                "mount a",
                "unmount a force=false",
                "unmount a force=true"
            ]
        );
    }
}
