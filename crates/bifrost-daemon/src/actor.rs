//! The single actor task that owns all daemon state (contract §5, §8): the config, registry, runtimes, holds,
//! probes, previous verdicts, provider status and the event ring. No locks on domain state. The actor never awaits
//! I/O and never touches a mount path: driver calls run in executor tasks that report back through `Msg`.
//! `Msg`, `Tick`, `Deps` and the `spawn` signature are frozen (A3).

use crate::api::{ApiCmd, ApiError};
use crate::state::{self, State};
use bifrost_config::{Config, ConfigError, ProviderConfig, static_source};
use bifrost_core::api::{
    ActionDto, DriverDto, MachineDto, MountDto, ProviderDto, ReloadDto, StatusDto,
};
use bifrost_core::events::{Event, EventRecord};
use bifrost_core::policy::Verdict;
use bifrost_core::reconcile::{
    self, Action, Availability, Candidate, DesiredInput, Health, MountRuntime, Phase, PlanInput,
    Reason, Timing,
};
use bifrost_core::registry::{Machine, MachineRegistry};
use bifrost_core::validate::{clean, random_u64};
use bifrost_core::{
    DiscoveryError, DiscoveryProvider, DriverAvailability, DriverSelector, MachineId,
    MachineObservation, MountDriver, MountError, MountHandle, MountId, MountRequest, MountState,
    Name, OnExit,
};
use bifrost_mount::DriverSettings;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::{Notify, broadcast, mpsc, oneshot, watch};
use tokio::task::{JoinHandle, JoinSet};
use tracing::{Instrument, debug_span, info, info_span, warn};

pub enum Msg {
    Discovery {
        provider: String,
        task_gen: u64,
        result: Result<Vec<MachineObservation>, DiscoveryError>,
    },
    MountDone {
        id: MountId,
        generation: u64,
        result: Result<MountHandle, MountError>,
    },
    UnmountDone {
        id: MountId,
        generation: u64,
        why: Reason,
        result: Result<(), MountError>,
    },
    Health {
        id: MountId,
        generation: u64,
        state: MountState,
    },
    ChildExited {
        id: MountId,
        generation: u64,
        detail: String,
    },
    Probed(BTreeMap<String, DriverAvailability>),
    Config {
        result: Result<Box<Config>, Vec<ConfigError>>,
        reply: Option<oneshot::Sender<ReloadDto>>,
    },
    Api(ApiCmd),
    Tick(Tick),
    /// Abort provider tasks, wait ≤5s for in-flight executors, write state.json, then reply (contract §8 shutdown).
    Shutdown(oneshot::Sender<()>),
}

pub enum Tick {
    Health,
    Fallback,
}

#[allow(clippy::type_complexity)] // frozen shape (A5)
pub struct Deps {
    /// Factory, not a Vec (A5): a reload that changes driver settings rebuilds through it, so daemon tests keep their FakeDrivers.
    pub drivers: Arc<dyn Fn(&DriverSettings) -> Vec<Arc<dyn MountDriver>> + Send + Sync>,
    pub build_provider:
        Arc<dyn Fn(&ProviderConfig) -> Result<Arc<dyn DiscoveryProvider>, String> + Send + Sync>,
}

/// `cfg_loaded`: false ⇒ `cfg` is the empty default because no config file exists yet; `ready` stays false until a
/// `Msg::Config` with Ok is applied (A6). The grace window runs from `cfg_loaded_at`, the first successful config
/// load (spawn time when `cfg_loaded`, else when that first `Msg::Config` applies), not from spawn. `socket` fills
/// `StatusDto.socket` (B11). The startup config goes through the same apply path as a reload (A4), and the first
/// pass runs before this returns, so the snapshot is never a placeholder.
#[allow(clippy::too_many_arguments)]
pub fn spawn(
    cfg: Config,
    cfg_loaded: bool,
    root: PathBuf,
    state_dir: PathBuf,
    socket: PathBuf,
    held: BTreeSet<MountId>,
    adopted: Vec<MountHandle>,
    deps: Deps,
) -> (
    mpsc::UnboundedSender<Msg>,
    watch::Receiver<Arc<StatusDto>>,
    broadcast::Sender<EventRecord>,
    JoinHandle<()>,
) {
    let (tx, rx) = mpsc::unbounded_channel();
    let (events, _) = broadcast::channel(256);
    let now = Instant::now();
    let mut a = Actor {
        cfg: Arc::new(cfg.clone()),
        cfg_loaded_at: None,
        ready: false,
        root,
        state_dir,
        socket,
        started: now,
        passed_at: now,
        deps,
        tx: tx.clone(),
        events: events.clone(),
        registry: MachineRegistry::default(),
        runtimes: (adopted.into_iter())
            .map(|h| (h.id.clone(), MountRuntime::adopted(h)))
            .collect(),
        created: 0,
        held,
        drivers: BTreeMap::new(),
        settings: None,
        probes: BTreeMap::new(),
        providers: BTreeMap::new(),
        tickers: vec![],
        tick_every: None,
        allowed: BTreeSet::new(),
        machines: vec![],
        candidates: BTreeMap::new(),
        conflicts: vec![],
        plan: vec![],
        config_errors: vec![],
        ring: VecDeque::new(),
        seq: 0,
        execs: JoinSet::new(),
        reconciles: vec![],
        answer: vec![],
        saved: None,
    };
    a.apply(Ok(Box::new(cfg)), now, Some(cfg_loaded));
    a.health_round(); // adopted mounts are inspected at t = 0
    a.pass(now);
    let (status, snapshot) = watch::channel(Arc::new(a.snapshot(now)));
    let join = tokio::spawn(a.run(rx, status));
    (tx, snapshot, events, join)
}

/// A task that dies with its owner: dropping a provider or a ticker aborts it.
struct Task(JoinHandle<()>);

impl Drop for Task {
    fn drop(&mut self) {
        self.0.abort();
    }
}

struct Provider {
    cfg: ProviderConfig,
    task_gen: u64,
    /// None when build_provider failed: no task, so warm-up waits for the grace period (B11); the next apply,
    /// an unchanged reload included, or the next fallback tick retries the build
    task: Option<Task>,
    notify: Arc<Notify>,
    refreshes: u64,
    last_ok: Option<Instant>,
    last_error: Option<String>,
    /// a non-empty Ok arrived at least once (A18)
    reported: bool,
}

struct Actor {
    cfg: Arc<Config>,
    /// the first successful config load; None while running the empty default of a missing file (A6)
    cfg_loaded_at: Option<Instant>,
    /// latches (§5 Warm-up)
    ready: bool,
    /// canonical
    root: PathBuf,
    state_dir: PathBuf,
    socket: PathBuf,
    started: Instant,
    /// the `now` of the last pass: deadlines are measured from it, so none falls between a pass and the sleep
    passed_at: Instant,
    deps: Deps,
    tx: mpsc::UnboundedSender<Msg>,
    events: broadcast::Sender<EventRecord>,
    registry: MachineRegistry,
    runtimes: BTreeMap<MountId, MountRuntime>,
    /// runtimes created so far: seeds a new runtime's generation, so no late result of a dropped one can match it
    created: u64,
    held: BTreeSet<MountId>,
    drivers: BTreeMap<String, Arc<dyn MountDriver>>,
    settings: Option<DriverSettings>,
    probes: BTreeMap<String, DriverAvailability>,
    providers: BTreeMap<String, Provider>,
    tickers: Vec<Task>,
    tick_every: Option<(Duration, Duration)>,
    /// machines Allowed at the previous pass (B1)
    allowed: BTreeSet<MachineId>,
    /// the last pass's view, for the snapshot and API targets
    machines: Vec<Machine>,
    candidates: BTreeMap<MountId, Candidate>,
    conflicts: Vec<String>,
    plan: Vec<(MountId, Action)>,
    config_errors: Vec<String>,
    ring: VecDeque<EventRecord>,
    seq: u64,
    execs: JoinSet<()>,
    /// POST /v1/reconcile replies waiting for their re-probe, then (`answer`) for the pass after it
    reconciles: Vec<oneshot::Sender<Vec<ActionDto>>>,
    answer: Vec<oneshot::Sender<Vec<ActionDto>>>,
    /// what state.json holds now
    saved: Option<State>,
}

async fn sleep_until(t: Option<Instant>) {
    match t {
        Some(t) => tokio::time::sleep_until(t.into()).await,
        None => std::future::pending().await,
    }
}

/// Health / fallback ticker (§5 Triggers): first tick one interval after (re)start.
fn ticker(tx: &mpsc::UnboundedSender<Msg>, every: Duration, tick: fn() -> Tick) -> Task {
    let tx = tx.clone();
    Task(tokio::spawn(async move {
        let start = tokio::time::Instant::now() + every;
        let mut i = tokio::time::interval_at(start, every);
        i.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            i.tick().await;
            if tx.send(Msg::Tick(tick())).is_err() {
                return;
            }
        }
    }))
}

/// One network provider (§5 Triggers): discover first (A7), then wait an interval or a POST /v1/discover.
// ponytail: discovery intervals have no jitter (synchronised provider polls), and one `failures` counter covers both mount and unmount; ±10% jitter
async fn discover_loop(
    p: Arc<dyn DiscoveryProvider>,
    provider: String,
    task_gen: u64,
    every: Duration,
    notify: Arc<Notify>,
    tx: mpsc::UnboundedSender<Msg>,
) {
    loop {
        let discover = caught(
            "provider",
            || p.discover(),
            |e| Err(DiscoveryError::Failed(e)),
        );
        let result = (tokio::time::timeout(Duration::from_secs(30), discover).await)
            .unwrap_or_else(|_| Err(DiscoveryError::Failed("timed out after 30s".into())));
        let provider = provider.clone();
        let msg = Msg::Discovery {
            provider,
            task_gen,
            result,
        };
        if tx.send(msg).is_err() {
            return;
        }
        tokio::select! {
            _ = tokio::time::sleep(every) => {}
            _ = notify.notified() => {}
        }
    }
}

/// Runs a driver or provider call. A panic in it, at call or at poll time, becomes `on_panic("<what> panicked:
/// <message>")` instead of killing the task, so the message the actor waits for is always sent and no runtime
/// stays in flight (S3 sign-off 2a).
async fn caught<T, F: Future<Output = T>>(
    what: &str,
    call: impl FnOnce() -> F,
    on_panic: impl FnOnce(String) -> T,
) -> T {
    use futures_util::FutureExt;
    let lazy = std::panic::AssertUnwindSafe(async move { call().await });
    lazy.catch_unwind().await.unwrap_or_else(|p| {
        let msg = (p.downcast_ref::<&str>().map(|s| s.to_string()))
            .or_else(|| p.downcast_ref::<String>().cloned());
        on_panic(format!(
            "{what} panicked: {}",
            clean(&msg.unwrap_or_default(), 512)
        ))
    })
}

/// Sign-off 2: MountRuntime stores no id, so the actor stamps the id it keys the runtime by into every event.
fn stamp(mut ev: Event, id: &MountId) -> Event {
    use Event::*;
    match &mut ev {
        MountRequested { mount, .. }
        | MountStarted { mount, .. }
        | MountHealthy { mount }
        | MountDegraded { mount, .. }
        | MountFailed { mount, .. }
        | UnmountStarted { mount, .. }
        | UnmountComplete { mount } => *mount = id.as_str().into(),
        _ => {}
    }
    ev
}

fn api_err(status: u16, error: String) -> ApiError {
    ApiError { status, error }
}

impl Actor {
    /// select! over the inbox, the earliest deadline and finished executors; drain the inbox, then one pass.
    // ponytail: no explicit wake detector (after a resume, probes and discovery happen within one interval); compare SystemTime vs Instant elapsed and force ticks
    async fn run(
        mut self,
        mut rx: mpsc::UnboundedReceiver<Msg>,
        status: watch::Sender<Arc<StatusDto>>,
    ) {
        loop {
            let first = tokio::select! {
                m = rx.recv() => m,
                () = sleep_until(self.deadline()) => None,
                Some(r) = self.execs.join_next() => {
                    if let Err(e) = r {
                        // driver panics are caught inside the task and reported as MountDone/UnmountDone Err
                        warn!("executor task failed: {e}");
                    }
                    continue;
                }
            };
            let msgs: Vec<Msg> = (first.into_iter())
                .chain(std::iter::from_fn(|| rx.try_recv().ok()))
                .collect();
            let now = Instant::now();
            // the whole batch is handled first, so a MountDone queued behind a Shutdown still reaches state.json
            let mut stop = None;
            for m in msgs {
                match m {
                    Msg::Shutdown(reply) => stop = Some(reply),
                    m => self.handle(m, now),
                }
            }
            if let Some(reply) = stop {
                self.shutdown(&mut rx).await;
                let _ = reply.send(());
                return;
            }
            let now = Instant::now();
            self.pass(now);
            status.send_replace(Arc::new(self.snapshot(now)));
            if !self.answer.is_empty() {
                let dto: Vec<ActionDto> = (self.plan.iter())
                    .map(|(id, a)| ActionDto {
                        mount: id.as_str().into(),
                        action: a.to_string(),
                    })
                    .collect();
                for r in self.answer.drain(..) {
                    let _ = r.send(dto.clone());
                }
            }
        }
    }

    /// §8 graceful shutdown: abort providers and tickers, wait ≤5s for in-flight executors (keeping their
    /// results), write state.json. Mounts are left alone; the next start adopts them.
    async fn shutdown(&mut self, rx: &mut mpsc::UnboundedReceiver<Msg>) {
        self.providers.clear();
        self.tickers.clear();
        let execs = &mut self.execs;
        let wait = async { while execs.join_next().await.is_some() {} };
        let _ = tokio::time::timeout(Duration::from_secs(5), wait).await;
        let now = Instant::now();
        while let Ok(m) = rx.try_recv() {
            if matches!(m, Msg::MountDone { .. } | Msg::UnmountDone { .. }) {
                self.handle(m, now);
            }
        }
        self.persist();
    }

    fn handle(&mut self, m: Msg, now: Instant) {
        let t = self.timing();
        match m {
            Msg::Discovery {
                provider,
                task_gen,
                result,
            } => self.discovery(provider, task_gen, result, now),
            Msg::MountDone {
                id,
                generation,
                result,
            } => {
                let rt = self.runtimes.get_mut(&id);
                let ev = rt.and_then(|rt| rt.mount_done(generation, result, now, &t, random_u64()));
                self.emit_mount(&id, ev);
            }
            Msg::UnmountDone {
                id,
                generation,
                why,
                result,
            } => {
                let rt = self.runtimes.get_mut(&id);
                let ev = rt
                    .and_then(|rt| rt.unmount_done(generation, why, result, now, &t, random_u64()));
                self.emit_mount(&id, ev);
            }
            Msg::Health {
                id,
                generation,
                state,
            } => {
                let ev = self.runtimes.get_mut(&id).and_then(|rt| {
                    rt.probing = false; // at most one inspect is in flight per runtime
                    rt.health(generation, state, now, &t, random_u64())
                });
                self.emit_mount(&id, ev);
            }
            Msg::ChildExited {
                id,
                generation,
                detail,
            } => {
                info!(mount = id.as_str(), generation, %detail, "mount process exited");
                // no state change: probe it now; ignored while Unmounting (§5). Not generation-gated: a failed
                // unmount leaves Mounted at a newer generation, and a stale exit costs only one harmless inspect
                let rt = self.runtimes.get(&id);
                if rt.is_some_and(|rt| rt.phase == Phase::Mounted) {
                    self.inspect(&id);
                }
            }
            Msg::Probed(probes) => {
                // B1: DriverUnavailable on Available → Unavailable only
                for (name, a) in &probes {
                    let was = matches!(
                        self.probes.get(name),
                        Some(DriverAvailability::Available { .. })
                    );
                    if let DriverAvailability::Unavailable(reason) = a
                        && was
                    {
                        let (driver, reason) = (name.clone(), reason.clone());
                        self.emit(Event::DriverUnavailable { driver, reason });
                    }
                }
                self.probes = probes;
                self.answer.append(&mut self.reconciles);
            }
            Msg::Config { result, reply } => {
                let dto = self.apply(result, now, None);
                if let Some(r) = reply {
                    let _ = r.send(dto);
                }
            }
            Msg::Api(cmd) => self.api(cmd),
            Msg::Tick(Tick::Health) => self.health_round(),
            Msg::Tick(Tick::Fallback) => {
                // a failed build gets another try without waiting for a reload (B11 stays: no task until it builds)
                let mut ps = std::mem::take(&mut self.providers);
                for p in ps.values_mut().filter(|p| p.task.is_none()) {
                    p.task_gen += 1;
                    self.start_provider(p);
                }
                self.providers = ps;
                self.probe(); // the pass follows its Msg::Probed
            }
            Msg::Shutdown(_) => {} // run() takes it
        }
    }

    fn timing(&self) -> Timing {
        let t = &self.cfg.timings;
        Timing {
            grace: t.offline_grace_period,
            retry_initial: t.retry_initial,
            retry_max: t.retry_max,
        }
    }

    /// The one config-apply path, for startup and every `Msg::Config` (A4). `startup = Some(cfg_loaded)`: the config
    /// counts as loaded only when a file was read, and no ConfigurationReloaded is emitted.
    fn apply(
        &mut self,
        result: Result<Box<Config>, Vec<ConfigError>>,
        now: Instant,
        startup: Option<bool>,
    ) -> ReloadDto {
        let result = match result {
            // A22: the actor does no I/O, so it can't create or canonicalize another root
            Ok(c) if c.root != self.cfg.root => Err(vec![ConfigError {
                path: c.path.display().to_string(),
                message: "mount.root change requires restart".into(),
            }]),
            r => r,
        };
        let new = match result {
            Ok(c) => *c,
            Err(errs) => {
                let errors: Vec<String> = errs.iter().map(ToString::to_string).collect();
                warn!(
                    "config rejected, keeping the active one: {}",
                    errors.join("; ")
                );
                self.config_errors = errors.clone();
                let ev = Event::ConfigurationReloaded {
                    ok: false,
                    errors: errors.clone(),
                };
                self.emit(ev);
                return ReloadDto { ok: false, errors };
            }
        };
        // ponytail: static discovery is registry.replace(config.static_observations()) with no provider object or task (static machines can only come from config); implement DiscoveryProvider if static ever needs another source
        let (added, gone) = (self.registry).replace(&static_source(), new.static_observations());
        self.discovered(added, "static");
        self.lost(gone);
        // providers, diffed by name: added → spawn; changed → respawn with task_gen + 1 (its observations stay
        // until they expire); unchanged → kept; removed → dropped (aborts the task) with its observations
        let mut old = std::mem::take(&mut self.providers);
        for pc in &new.providers {
            let mut p = old.remove(&pc.name).unwrap_or_else(|| Provider {
                cfg: pc.clone(),
                task_gen: 0,
                task: None,
                notify: Arc::default(),
                refreshes: 0,
                last_ok: None,
                last_error: None,
                reported: false,
            });
            if p.task.is_none() || p.cfg != *pc {
                p.cfg = pc.clone();
                p.task_gen += 1;
                self.start_provider(&mut p);
            }
            self.providers.insert(pc.name.clone(), p);
        }
        for name in old.into_keys() {
            let gone = self.registry.remove_provider(&name);
            self.lost(gone);
        }
        // drivers: rebuilt through the factory only when their settings change (A5); re-probed on every apply
        let settings = DriverSettings {
            ssh_config: new.ssh_config.clone(),
            vfs_cache_mode: new.vfs_cache_mode.clone(),
            mount_timeout: new.timings.mount_timeout,
            state_dir: self.state_dir.clone(),
        };
        if self.settings.as_ref() != Some(&settings) {
            self.drivers = ((self.deps.drivers)(&settings).into_iter())
                .map(|d| (d.name().to_string(), d))
                .collect();
            self.settings = Some(settings);
        }
        self.probe();
        let every = (new.timings.health_interval, new.timings.reconcile_interval);
        if self.tick_every != Some(every) {
            self.tickers = vec![
                ticker(&self.tx, every.0, || Tick::Health),
                ticker(&self.tx, every.1, || Tick::Fallback),
            ];
            self.tick_every = Some(every);
        }
        self.cfg = Arc::new(new);
        self.config_errors.clear();
        if startup.unwrap_or(true) {
            self.cfg_loaded_at.get_or_insert(now); // A6: never for the empty default of a missing file
        }
        if startup.is_none() {
            let ev = Event::ConfigurationReloaded {
                ok: true,
                errors: vec![],
            };
            self.emit(ev);
        }
        ReloadDto {
            ok: true,
            errors: vec![],
        }
    }

    fn start_provider(&mut self, p: &mut Provider) {
        p.task = None; // aborts the previous task
        match (self.deps.build_provider)(&p.cfg) {
            Ok(d) => {
                let span = info_span!("discovery", provider = %p.cfg.name, kind = %p.cfg.kind);
                let (name, tx, notify) = (p.cfg.name.clone(), self.tx.clone(), p.notify.clone());
                let run = discover_loop(d, name, p.task_gen, p.cfg.interval, notify, tx);
                p.task = Some(Task(tokio::spawn(run.instrument(span))));
            }
            Err(e) => {
                p.last_error = Some(clean(&e, 512)); // B11
                // a changed provider's kept observations freeze like a failed refresh's, not age out (PRD §27)
                self.registry.mark_failed(&p.cfg.name);
            }
        }
    }

    fn discovery(
        &mut self,
        provider: String,
        task_gen: u64,
        result: Result<Vec<MachineObservation>, DiscoveryError>,
        now: Instant,
    ) {
        // a removed provider's, or an aborted task's, late result
        let Some(p) = (self.providers.get_mut(&provider)).filter(|p| p.task_gen == task_gen) else {
            return;
        };
        p.refreshes += 1;
        let obs = match result {
            Ok(obs) => {
                p.last_ok = Some(now);
                p.last_error = None;
                p.reported |= !obs.is_empty(); // an empty Ok never counts toward ready (A18)
                obs
            }
            Err(e) => {
                let e = clean(&e.to_string(), 512);
                if p.last_error.as_ref() != Some(&e) {
                    warn!(provider, "discovery failed: {e}");
                }
                p.last_error = Some(e);
                self.registry.mark_failed(&provider); // freeze, not drop
                return;
            }
        };
        // removal hysteresis: an absent machine ages out after 3 × interval
        let (src, floor) = (p.cfg.source(), p.cfg.interval.saturating_mul(3));
        let added = self.registry.apply_ok(&src, obs, now, floor);
        self.discovered(added, &provider);
    }

    fn discovered(&mut self, ids: Vec<MachineId>, provider: &str) {
        for id in ids {
            let (machine, provider) = (id.as_str().into(), provider.into());
            self.emit(Event::MachineDiscovered { machine, provider });
        }
    }

    fn lost(&mut self, ids: Vec<MachineId>) {
        for id in ids {
            let machine = id.as_str().into();
            self.emit(Event::MachineLost { machine });
        }
    }

    /// Re-probes every driver concurrently; the result comes back as `Msg::Probed`.
    // ponytail: drivers are re-probed only at startup, reload, the fallback tick and POST /v1/reconcile (a freshly installed tool is seen within reconcile_interval)
    fn probe(&self) {
        let (drivers, tx) = (self.drivers.clone(), self.tx.clone());
        tokio::spawn(async move {
            let each = (drivers.iter()).map(|(n, d)| async move {
                let unavailable = DriverAvailability::Unavailable;
                (n.clone(), caught("driver", || d.probe(), unavailable).await)
            });
            let probes = futures_util::future::join_all(each).await;
            let _ = tx.send(Msg::Probed(probes.into_iter().collect()));
        });
    }

    fn api(&mut self, cmd: ApiCmd) {
        match cmd {
            ApiCmd::Mount { target, reply } => {
                let _ = reply.send(self.api_mount(&target));
            }
            ApiCmd::Unmount {
                target,
                force,
                reply,
            } => {
                let _ = reply.send(self.api_unmount(&target, force));
            }
            // re-probe, then run a pass and return its plan (answered after that pass). It leaves
            // mount_retry_at alone, so a second call has no side effects (A11); POST …/mount is "retry now".
            ApiCmd::Reconcile { reply } => {
                self.reconciles.push(reply);
                self.probe();
            }
            ApiCmd::Discover => self.providers.values().for_each(|p| p.notify.notify_one()),
        }
    }

    /// A mount id (known mount or hold) wins over a machine id (B15); a machine id means all its mounts.
    fn targets(&self, target: &str) -> Result<Vec<MountId>, ApiError> {
        let unknown = || api_err(404, format!("unknown target {}", clean(target, 128)));
        let id = Name::parse(target).map_err(|_| unknown())?;
        let known = self.candidates.contains_key(&id) || self.runtimes.contains_key(&id);
        if known || self.held.contains(&id) {
            return Ok(vec![id]);
        }
        if !self.machines.iter().any(|m| m.id == id) {
            return Err(unknown());
        }
        let of_machine = self.candidates.values().filter(|c| c.spec.machine == id);
        Ok(of_machine.map(|c| c.spec.id.clone()).collect())
    }

    /// "Retry now": clears the hold, both retry timers, `offline`, `failures` (§5). Never overrides policy: 403
    /// with the verdict for anything that isn't a candidate (§4).
    fn api_mount(&mut self, target: &str) -> Result<Vec<String>, ApiError> {
        let ids = self.targets(target)?;
        if ids.is_empty() || ids.iter().any(|id| !self.candidates.contains_key(id)) {
            let id = Name::parse(target).map_err(|e| api_err(404, e.to_string()))?;
            return Err(api_err(
                403,
                format!("{}: {}", id.as_str(), self.verdict_of(&id)),
            ));
        }
        for id in &ids {
            self.held.remove(id);
            if let Some(rt) = self.runtimes.get_mut(id) {
                rt.mount_retry_at = None;
                rt.unmount_retry_at = None;
                rt.offline = false;
                rt.failures = 0;
                rt.force_requested = false;
            }
        }
        Ok(ids.iter().map(|i| i.as_str().to_string()).collect())
    }

    /// The verdict of the machine named `id`, or of the static machine owning mount `id`.
    fn verdict_of(&self, id: &MountId) -> String {
        let owner = (self.cfg.machines.iter())
            .find(|m| m.mounts.iter().any(|s| s.local == *id))
            .map_or(id, |m| &m.id);
        (self.machines.iter())
            .find(|m| m.id == *owner && !m.verdict.is_allowed())
            .map_or_else(|| "not a candidate".into(), |m| m.verdict.to_string())
    }

    /// Adds a persisted hold; `force` asks for a lazy detach (row 3).
    fn api_unmount(&mut self, target: &str, force: bool) -> Result<Vec<String>, ApiError> {
        let ids = self.targets(target)?;
        if ids.is_empty() {
            return Err(api_err(404, format!("{}: no mounts", clean(target, 128))));
        }
        for id in &ids {
            self.held.insert(id.clone());
            if let Some(rt) = self.runtimes.get_mut(id) {
                rt.force_requested |= force;
            }
        }
        Ok(ids.iter().map(|i| i.as_str().to_string()).collect())
    }

    /// A6 + A18: false until a config file has loaded; then true once every network provider has sent a
    /// non-empty Ok, or the grace period has passed since that load. Latches.
    fn ready(&mut self, now: Instant) -> bool {
        let grace = self.cfg.timings.offline_grace_period;
        let all = self.providers.values().all(|p| p.reported);
        self.ready |= self.cfg_loaded_at.is_some_and(|t| all || now >= t + grace);
        self.ready
    }

    /// The earliest instant a pass could act on (next_wakeup, or the end of warm-up). Only instants after the last
    /// pass count: one that passed without its row acting can't make the actor spin, and every wake runs a pass
    /// that moves `passed_at` past the deadline that woke it.
    fn deadline(&self) -> Option<Instant> {
        let now = self.passed_at;
        let grace = self.cfg.timings.offline_grace_period;
        let warmup = self
            .cfg_loaded_at
            .filter(|_| !self.ready)
            .map(|t| t + grace);
        (reconcile::next_wakeup(&self.runtimes, grace, now).into_iter())
            .chain(warmup)
            .filter(|t| *t > now)
            .min()
    }

    /// One pass (§5): expire → machines → desired → plan → execute; then state.json if it changed.
    fn pass(&mut self, now: Instant) {
        self.passed_at = now;
        let gone = self.registry.expire(now);
        self.lost(gone);
        let machines = self.registry.machines(&self.cfg.policy);
        // B1: MachineEligible when a machine becomes Allowed
        let mut allowed = BTreeSet::new();
        let mut eligible = vec![];
        for m in &machines {
            if let Verdict::Allowed { by } = &m.verdict {
                if !self.allowed.contains(&m.id) {
                    let (machine, via) = (m.id.as_str().into(), by.clone());
                    eligible.push(Event::MachineEligible { machine, via });
                }
                allowed.insert(m.id.clone());
            }
        }
        self.allowed = allowed;
        eligible.into_iter().for_each(|e| self.emit(e));
        let desired = reconcile::desired(&DesiredInput {
            root: &self.root,
            machines: &machines,
            static_mounts: &self.cfg.static_mounts(),
            templates: &self.cfg.templates(),
            held: &self.held,
            auto_order: &self.cfg.auto_order,
            probes: &self.probes,
        });
        self.machines = machines;
        self.candidates = desired.candidates;
        self.conflicts = desired.conflicts;
        let plan = reconcile::plan(&PlanInput {
            now,
            ready: self.ready(now),
            grace: self.cfg.timings.offline_grace_period,
            candidates: &self.candidates,
            runtimes: &self.runtimes,
        });
        for (id, a) in &plan {
            match a {
                Action::Mount { driver } => self.start_mount(id, driver),
                Action::Unmount { force, why } | Action::Remount { force, why } => {
                    self.start_unmount(id, *force, *why, a)
                }
                _ => {}
            }
        }
        // row 2: an Absent runtime that is not desired is dropped once no retry is pending
        let c = &self.candidates;
        self.runtimes.retain(|id, rt| {
            rt.phase != Phase::Absent
                || rt.mount_retry_at.is_some_and(|t| t > now)
                || c.get(id).is_some_and(|c| !c.held)
        });
        self.plan = plan;
        self.persist();
    }

    // ponytail: no concurrency cap on mount operations (dozens of simultaneous auto-mounts each spawn ssh at once); a tokio Semaphore in the executor
    fn start_mount(&mut self, id: &MountId, driver: &str) {
        let (Some(c), Some(d)) = (self.candidates.get(id), self.drivers.get(driver)) else {
            return;
        };
        let (spec, d) = (c.spec.clone(), d.clone());
        let provider = (self.machines.iter())
            .find(|m| m.id == spec.machine)
            .map_or(String::new(), |m| m.source().provider.clone());
        let created = &mut self.created;
        let rt = self.runtimes.entry(id.clone()).or_insert_with(|| {
            *created += 1;
            MountRuntime {
                generation: *created << 32,
                ..MountRuntime::default()
            }
        });
        let generation = rt.begin(Phase::Mounting);
        let span = info_span!("mount_op", machine_id = spec.machine.as_str(), mount_id = id.as_str(),
            %provider, driver, local_path = %spec.local_path.display(), remote_path = spec.remote.as_str(),
            attempt = rt.failures + 1);
        let ev = Event::MountRequested {
            mount: String::new(),
            driver: driver.into(),
        };
        self.emit_mount(id, Some(ev));
        let (tx, exit_tx, exit_id) = (self.tx.clone(), self.tx.clone(), id.clone());
        // called by the driver's supervisor when the child exits; must never block (unbounded send)
        let on_exit: OnExit = Box::new(move |detail| {
            let (id, generation) = (exit_id, generation);
            let _ = exit_tx.send(Msg::ChildExited {
                id,
                generation,
                detail,
            });
        });
        let log_path = self.state_dir.join(format!("logs/{}.log", id.as_str()));
        let req = MountRequest {
            spec,
            log_path,
            on_exit,
        };
        // A9: covers the 15s preflight, mount_timeout and the driver's own 10s lazy detach
        let limit = self.cfg.timings.mount_timeout + Duration::from_secs(60);
        let id = id.clone();
        let op = async move {
            let mount = caught("driver", || d.mount(req), |e| Err(MountError::Failed(e)));
            let result = (tokio::time::timeout(limit, mount).await).unwrap_or_else(|_| {
                Err(MountError::Failed(format!(
                    "timed out after {}s",
                    limit.as_secs()
                )))
            });
            let _ = tx.send(Msg::MountDone {
                id,
                generation,
                result,
            });
        };
        self.execs.spawn(op.instrument(span));
    }

    /// Unmount and Remount alike (§5: the new mount follows via row 9 on a later pass).
    fn start_unmount(&mut self, id: &MountId, force: bool, why: Reason, a: &Action) {
        let Some(rt) = self.runtimes.get_mut(id) else {
            return;
        };
        let Some(h) = rt.handle.clone() else {
            return;
        };
        let generation = rt.begin(Phase::Unmounting);
        let span = info_span!("mount_op", mount_id = id.as_str(), driver = h.driver.as_str(),
            local_path = %h.local_path.display(), attempt = rt.failures + 1);
        let d = self.drivers.get(&h.driver).cloned();
        let ev = Event::UnmountStarted {
            mount: String::new(),
            reason: a.to_string(),
        };
        self.emit_mount(id, Some(ev));
        let (tx, id) = (self.tx.clone(), id.clone());
        let op = async move {
            let unmount = async {
                match &d {
                    Some(d) => d.unmount(&h, force).await,
                    None => bifrost_mount::unmount_path(&h.local_path, force).await,
                }
            };
            let unmount = caught("driver", || unmount, |e| Err(MountError::Failed(e)));
            let result = (tokio::time::timeout(Duration::from_secs(30), unmount).await)
                .unwrap_or_else(|_| Err(MountError::Failed("unmount timed out after 30s".into())));
            // OfflineGrace too: row 7 keeps an offline peer unmounted, so no failed mount cleans up after a peer
            // that is then deleted; the next mount recreates the directory (prepare_mountpoint)
            let rm_dir = matches!(
                why,
                Reason::NotDesired | Reason::Manual | Reason::OfflineGrace
            );
            if result.is_ok() && rm_dir {
                // §6: only an empty directory, only once the table says it is no mountpoint; never recursive
                let p = h.local_path.clone();
                let rm = move || {
                    use bifrost_mount::table;
                    if table::read().is_ok_and(|t| table::find(&t, &p).is_none()) {
                        let _ = std::fs::remove_dir(&p);
                    }
                };
                let _ = tokio::task::spawn_blocking(rm).await;
            }
            let _ = tx.send(Msg::UnmountDone {
                id,
                generation,
                why,
                result,
            });
        };
        self.execs.spawn(op.instrument(span));
    }

    fn health_round(&mut self) {
        let ids: Vec<MountId> = self.runtimes.keys().cloned().collect();
        ids.iter().for_each(|id| self.inspect(id));
    }

    /// Inspects a Mounted runtime unless an inspect is already in flight.
    fn inspect(&mut self, id: &MountId) {
        let Some(rt) = self.runtimes.get_mut(id) else {
            return;
        };
        let Some(h) = (rt.handle.clone()).filter(|_| rt.phase == Phase::Mounted && !rt.probing)
        else {
            return;
        };
        let Some(d) = self.drivers.get(&h.driver).cloned() else {
            return; // no driver object for an adopted handle's driver: nothing to inspect with
        };
        rt.probing = true;
        let (generation, tx, id) = (rt.generation, self.tx.clone(), id.clone());
        let span = debug_span!("health", mount_id = id.as_str());
        tokio::spawn(
            async move {
                // drivers answer within ~5s even on a hung FUSE mount; this cap only guards a driver bug
                let limit = Duration::from_secs(30);
                let inspect = caught("driver", || d.inspect(&h), MountState::Degraded);
                let state = (tokio::time::timeout(limit, inspect).await)
                    .unwrap_or_else(|_| MountState::Degraded("inspect timed out".into()));
                let _ = tx.send(Msg::Health {
                    id,
                    generation,
                    state,
                });
            }
            .instrument(span),
        );
    }

    fn emit(&mut self, event: Event) {
        info!(?event);
        self.seq += 1;
        let ts = SystemTime::now().duration_since(UNIX_EPOCH);
        let r = EventRecord {
            seq: self.seq,
            ts_unix_ms: ts.map_or(0, |d| d.as_millis() as u64),
            event,
        };
        if self.ring.len() == 200 {
            self.ring.pop_front();
        }
        self.ring.push_back(r.clone());
        let _ = self.events.send(r);
    }

    fn emit_mount(&mut self, id: &MountId, ev: Option<Event>) {
        if let Some(ev) = ev {
            self.emit(stamp(ev, id));
        }
    }

    /// state.json whenever `held` or the mount handles change (§8).
    // ponytail: state.json is written synchronously on the actor (a few hundred bytes to the local state dir); spawn_blocking with ordered writes if the state dir is ever slow
    fn persist(&mut self) {
        let mounts = (self.runtimes.iter())
            .filter_map(|(id, rt)| Some((id.clone(), rt.handle.clone()?)))
            .collect();
        let s = State {
            version: 1,
            held: self.held.clone(),
            mounts,
        };
        if self.saved.as_ref() == Some(&s) {
            return;
        }
        match state::write(&self.state_dir, &s) {
            Ok(()) => self.saved = Some(s),
            Err(e) => warn!("{}/state.json: {e}", self.state_dir.display()),
        }
    }

    /// GET /v1/status (§2, §8).
    fn snapshot(&self, now: Instant) -> StatusDto {
        let secs = |t: Option<Instant>| {
            let d = t
                .and_then(|t| t.checked_duration_since(now))
                .filter(|d| !d.is_zero());
            d.map(|d| d.as_millis().div_ceil(1000) as u64)
        };
        let plan: BTreeMap<&MountId, &Action> = self.plan.iter().map(|(i, a)| (i, a)).collect();
        let none = MountRuntime::default();
        let ids: BTreeSet<&MountId> = self.candidates.keys().chain(self.runtimes.keys()).collect();
        let mounts: Vec<MountDto> = (ids.into_iter())
            .map(|id| {
                let (c, rt) = (self.candidates.get(id), self.runtimes.get(id));
                let r = rt.unwrap_or(&none);
                let h = r.handle.as_ref();
                // before the first probe result (drivers "probing") a driver Err means not probed yet: the mount
                // is pending, not Failed, so `bifrost mount`/doctor right after start don't report a false failure
                let unprobed = (self.probes.is_empty() && !self.drivers.is_empty())
                    && r.phase == Phase::Absent
                    && c.is_some_and(|c| !c.held && c.driver.is_err());
                let state = match reconcile::mount_availability(rt, c) {
                    Availability::Failed if unprobed => Availability::Eligible,
                    s => s,
                };
                let detail = match &r.health {
                    Health::Degraded(s) | Health::Stale(s) if r.phase == Phase::Mounted => {
                        s.clone()
                    }
                    _ if unprobed => "probing drivers".into(),
                    _ => (c.filter(|c| !c.held).and_then(|c| c.driver.clone().err()))
                        .or_else(|| r.last_error.clone())
                        .unwrap_or_default(),
                };
                let local = h
                    .map(|h| h.local_path.clone())
                    .or(c.map(|c| c.spec.local_path.clone()));
                let retry = if r.phase == Phase::Mounted {
                    r.unmount_retry_at
                } else {
                    r.mount_retry_at
                };
                MountDto {
                    id: id.as_str().into(),
                    machine: c.map_or(String::new(), |c| c.spec.machine.as_str().into()),
                    driver: h
                        .map(|h| h.driver.clone())
                        .or(c.and_then(|c| c.driver.clone().ok())),
                    local_path: local
                        .unwrap_or_else(|| self.root.join(id.as_str()))
                        .display()
                        .to_string(),
                    remote: c.map_or(String::new(), |c| c.spec.source()),
                    state,
                    detail,
                    desired: c.is_some_and(|c| !c.held),
                    held: self.held.contains(id),
                    adopted: r.adopted,
                    pid: h.and_then(|h| h.pid),
                    failures: r.failures,
                    retry_in_secs: secs(retry),
                    last_error: r.last_error.clone(),
                    action: plan.get(id).map_or("noop".into(), |a| a.to_string()),
                }
            })
            .collect();
        let state: BTreeMap<&str, Availability> =
            mounts.iter().map(|m| (m.id.as_str(), m.state)).collect();
        let machines = (self.machines.iter())
            .map(|m| {
                let o = m.obs();
                let ids: Vec<String> = (self.candidates.values())
                    .filter(|c| c.spec.machine == m.id)
                    .map(|c| c.spec.id.as_str().into())
                    .collect();
                let states: Vec<Availability> = ids
                    .iter()
                    .filter_map(|i| state.get(i.as_str()).copied())
                    .collect();
                MachineDto {
                    id: m.id.as_str().into(),
                    name: o.name.clone(),
                    source: m.source().provider.clone(),
                    shadowed: m.shadowed(),
                    address: o
                        .addresses
                        .first()
                        .map_or(String::new(), |a| a.as_str().into()),
                    port: o.port,
                    online: o.online,
                    tags: o.metadata.tags.iter().cloned().collect(),
                    metadata: o.metadata.values.clone(),
                    verdict: m.verdict.to_string(),
                    state: reconcile::machine_availability(m, &states),
                    mounts: ids,
                }
            })
            .collect();
        // ponytail: provider warnings (skipped records) go only to the log, invisible from the CLI and TUI; `warnings` in ProviderDto
        let providers = (self.providers.values())
            .map(|p| ProviderDto {
                name: p.cfg.name.clone(),
                kind: p.cfg.kind.clone(),
                machines: (self.machines.iter())
                    .filter(|m| m.observed.iter().any(|o| o.source.provider == p.cfg.name))
                    .count(),
                refreshes: p.refreshes,
                last_ok_secs_ago: p
                    .last_ok
                    .map(|t| now.saturating_duration_since(t).as_secs()),
                last_error: p.last_error.clone(),
            })
            .collect();
        // every built driver, from the first snapshot on: "probing" until its first probe answers (S3 sign-off 2b)
        let probing = DriverAvailability::Unavailable("probing".into());
        let drivers = (self.drivers.keys())
            .map(|name| match self.probes.get(name).unwrap_or(&probing) {
                DriverAvailability::Available { binary, detail } => DriverDto {
                    name: name.clone(),
                    available: true,
                    binary: Some(binary.display().to_string()),
                    detail: detail.clone(),
                },
                DriverAvailability::Unavailable(why) => DriverDto {
                    name: name.clone(),
                    available: false,
                    binary: None,
                    detail: why.clone(),
                },
            })
            .collect();
        StatusDto {
            version: env!("CARGO_PKG_VERSION").into(),
            pid: std::process::id(),
            uptime_secs: now.saturating_duration_since(self.started).as_secs(),
            socket: self.socket.display().to_string(),
            config_path: self.cfg.path.display().to_string(),
            config_errors: self.config_errors.clone(),
            mount_root: self.root.display().to_string(),
            ready: self.ready,
            ssh_agent: std::env::var_os("SSH_AUTH_SOCK").is_some_and(|v| !v.is_empty()),
            providers,
            drivers,
            auto_driver: reconcile::select_driver(
                &DriverSelector::Auto,
                &self.cfg.auto_order,
                &self.probes,
            )
            .ok(),
            machines,
            mounts,
            conflicts: self.conflicts.clone(),
            events: self.ring.iter().cloned().collect(),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::api::ApiError;
    use bifrost_core::api::{ActionDto, MountDto};
    use bifrost_core::events::Event;
    use bifrost_core::fake::{FakeDiscovery, FakeDriver, obs};
    use bifrost_core::reconcile::Availability;
    use bifrost_core::{BoxFuture, MountRequest, Name};
    use std::path::Path;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering::SeqCst};
    use std::time::{Duration, Instant};

    /// A FakeDriver named "sshfs" (the default auto order picks it) that counts inspects and can slow unmounts.
    pub(crate) struct TestDriver {
        pub fake: FakeDriver,
        pub inspects: AtomicUsize,
        pub unmount_delay_ms: AtomicU64,
    }

    impl TestDriver {
        pub fn new() -> Self {
            Self {
                fake: FakeDriver::new("sshfs"),
                inspects: AtomicUsize::new(0),
                unmount_delay_ms: AtomicU64::new(0),
            }
        }
    }

    impl MountDriver for TestDriver {
        fn name(&self) -> &str {
            self.fake.name()
        }
        fn probe(&self) -> BoxFuture<'_, DriverAvailability> {
            self.fake.probe()
        }
        fn mount(&self, req: MountRequest) -> BoxFuture<'_, Result<MountHandle, MountError>> {
            self.fake.mount(req)
        }
        fn inspect<'a>(&'a self, h: &'a MountHandle) -> BoxFuture<'a, MountState> {
            self.inspects.fetch_add(1, SeqCst);
            self.fake.inspect(h)
        }
        fn unmount<'a>(
            &'a self,
            h: &'a MountHandle,
            force: bool,
        ) -> BoxFuture<'a, Result<(), MountError>> {
            let ms = self.unmount_delay_ms.load(SeqCst);
            Box::pin(async move {
                tokio::time::sleep(Duration::from_millis(ms)).await;
                self.fake.unmount(h, force).await
            })
        }
    }

    pub(crate) const RECON: &str =
        "offline_grace_period = \"1h\"\nretry_initial = \"1s\"\nretry_max = \"2s\"\n";
    /// retries far beyond any test's runtime
    const SLOW_RETRY: &str =
        "offline_grace_period = \"1h\"\nretry_initial = \"1m\"\nretry_max = \"2m\"\n";
    const FAKE: &str = "[[discovery]]\ntype = \"tailscale\"\nname = \"fake\"\n";
    const FAKE_ALL: &str = "[[discovery]]\ntype = \"tailscale\"\nname = \"fake\"\n\
                            [discovery.filter]\ninclude_names = [\"*\"]\n";
    const BUSY: &str = "unmount blocked: busy (files open)";

    pub(crate) fn machine(name: &str) -> String {
        format!("[[machines]]\nname = \"{name}\"\nhost = \"10.0.0.1\"\nremote = \"/srv\"\n")
    }

    fn id(s: &str) -> MountId {
        Name::parse(s).unwrap()
    }

    /// Everything a test daemon needs besides the actor: temp dirs, the fake driver and the fake provider.
    #[derive(Clone)]
    pub(crate) struct Rig {
        pub dir: PathBuf,
        pub root: PathBuf,
        pub drv: Arc<TestDriver>,
        pub disc: Arc<FakeDiscovery>,
        pub builds: Arc<AtomicUsize>,
        pub fail_build: Arc<AtomicBool>,
    }

    pub(crate) fn rig(name: &str) -> Rig {
        let dir = std::env::temp_dir().join(format!("bf-d-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("state/logs")).unwrap();
        Rig {
            root: dir.join("root"),
            dir,
            drv: Arc::new(TestDriver::new()),
            disc: Arc::new(FakeDiscovery {
                name: "fake".into(),
                result: Mutex::new(Err(DiscoveryError::Failed("no result set".into()))),
            }),
            builds: Arc::new(AtomicUsize::new(0)),
            fail_build: Arc::default(),
        }
    }

    impl Rig {
        /// daemon intervals of 1h: nothing ticks unless a test sends the Tick
        pub fn text(&self, root: &Path, recon: &str, extra: &str) -> String {
            format!(
                "[mount]\nroot = \"{}\"\n[daemon]\ndiscovery_interval = \"1h\"\nhealth_interval = \"1h\"\n\
                 reconcile_interval = \"1h\"\n[reconciliation]\n{recon}{extra}",
                root.display()
            )
        }
        pub fn cfg_at(&self, root: &Path, recon: &str, extra: &str) -> Config {
            let text = self.text(root, recon, extra);
            bifrost_config::parse(&text, &self.dir.join("config.toml"), &|_| None).unwrap()
        }
        pub fn cfg(&self, recon: &str, extra: &str) -> Config {
            self.cfg_at(&self.root, recon, extra)
        }
        pub fn deps(&self) -> Deps {
            let (drv, disc, builds) = (self.drv.clone(), self.disc.clone(), self.builds.clone());
            let fail = self.fail_build.clone();
            Deps {
                drivers: Arc::new(move |_| vec![drv.clone() as Arc<dyn MountDriver>]),
                build_provider: Arc::new(move |_| {
                    builds.fetch_add(1, SeqCst);
                    if fail.load(SeqCst) {
                        return Err("build failed".into());
                    }
                    Ok(disc.clone() as Arc<dyn DiscoveryProvider>)
                }),
            }
        }
        pub fn set_disc(&self, r: Result<Vec<MachineObservation>, DiscoveryError>) {
            *self.disc.result.lock().unwrap() = r;
        }
        pub fn calls(&self) -> Vec<String> {
            self.drv.fake.calls.lock().unwrap().clone()
        }
        /// `adopted`: marker mounts already in the (fake) mount table under the root.
        pub fn start(&self, cfg: Config, loaded: bool, held: &[&str], adopted: &[&str]) -> H {
            let adopted: Vec<MountHandle> = (adopted.iter())
                .map(|a| MountHandle {
                    id: id(a),
                    driver: "sshfs".into(),
                    local_path: self.root.join(a),
                    fingerprint: "0000000000000000".into(),
                    pid: None,
                })
                .collect();
            for h in &adopted {
                let mut m = self.drv.fake.mounted.lock().unwrap();
                m.insert(h.id.clone(), h.clone());
            }
            let held = held.iter().map(|h| id(h)).collect();
            let (tx, status, _, join) = spawn(
                cfg,
                loaded,
                self.root.clone(),
                self.dir.join("state"),
                self.dir.join("s.sock"),
                held,
                adopted,
                self.deps(),
            );
            H { tx, status, join }
        }
    }

    pub(crate) struct H {
        pub tx: mpsc::UnboundedSender<Msg>,
        pub status: watch::Receiver<Arc<StatusDto>>,
        pub join: JoinHandle<()>,
    }

    impl H {
        pub async fn until(
            &mut self,
            what: &str,
            f: impl Fn(&StatusDto) -> bool,
        ) -> Arc<StatusDto> {
            let wait = self.status.wait_for(|s| f(s));
            if let Ok(Ok(s)) = tokio::time::timeout(Duration::from_secs(5), wait).await {
                return s.clone();
            }
            panic!("timed out waiting for {what}: {:#?}", self.status.borrow())
        }
        pub fn send(&self, m: Msg) {
            assert!(self.tx.send(m).is_ok(), "actor gone");
        }
        pub async fn api<T>(&self, f: impl FnOnce(oneshot::Sender<T>) -> ApiCmd) -> T {
            let (tx, rx) = oneshot::channel();
            self.send(Msg::Api(f(tx)));
            tokio::time::timeout(Duration::from_secs(5), rx)
                .await
                .unwrap()
                .unwrap()
        }
        pub async fn mount(&self, t: &str) -> Result<Vec<String>, ApiError> {
            let target = t.to_string();
            self.api(|reply| ApiCmd::Mount { target, reply }).await
        }
        pub async fn unmount(&self, t: &str, force: bool) -> Result<Vec<String>, ApiError> {
            let target = t.to_string();
            (self.api(|reply| ApiCmd::Unmount {
                target,
                force,
                reply,
            }))
            .await
        }
        pub async fn reconcile(&self) -> Vec<ActionDto> {
            self.api(|reply| ApiCmd::Reconcile { reply }).await
        }
        pub async fn config(&self, result: Result<Box<Config>, Vec<ConfigError>>) -> ReloadDto {
            let (tx, rx) = oneshot::channel();
            self.send(Msg::Config {
                result,
                reply: Some(tx),
            });
            tokio::time::timeout(Duration::from_secs(5), rx)
                .await
                .unwrap()
                .unwrap()
        }
        pub async fn reload(&self, cfg: Config) -> ReloadDto {
            self.config(Ok(Box::new(cfg))).await
        }
        pub async fn shutdown(self) {
            let (tx, rx) = oneshot::channel();
            self.send(Msg::Shutdown(tx));
            tokio::time::timeout(Duration::from_secs(7), rx)
                .await
                .unwrap()
                .unwrap();
            tokio::time::timeout(Duration::from_secs(1), self.join)
                .await
                .unwrap()
                .unwrap();
        }
    }

    fn ok(r: Result<Vec<String>, ApiError>) -> Vec<String> {
        r.unwrap_or_else(|e| panic!("{}: {}", e.status, e.error))
    }

    pub(crate) fn mount<'a>(s: &'a StatusDto, id: &str) -> Option<&'a MountDto> {
        s.mounts.iter().find(|m| m.id == id)
    }

    pub(crate) fn is(s: &StatusDto, id: &str, a: Availability) -> bool {
        mount(s, id).is_some_and(|m| m.state == a)
    }

    /// A MountFailed event, not just the Failed state (which a failed driver selection also shows, B15).
    fn failed(s: &StatusDto, id: &str) -> bool {
        (s.events.iter())
            .any(|r| matches!(&r.event, Event::MountFailed { mount, .. } if mount == id))
    }

    fn events(s: &StatusDto) -> Vec<Event> {
        s.events.iter().map(|r| r.event.clone()).collect()
    }

    fn sorted(mut v: Vec<String>) -> Vec<String> {
        v.sort();
        v
    }

    #[tokio::test]
    async fn mounts_desired_on_startup_and_idempotent() {
        let r = rig("startup");
        let mut h = r.start(
            r.cfg(RECON, &(machine("a") + &machine("b"))),
            true,
            &[],
            &[],
        );
        let s = h
            .until("a and b mounted", |s| {
                is(s, "a", Availability::Mounted) && is(s, "b", Availability::Mounted)
            })
            .await;
        assert!(s.ready);
        assert_eq!(sorted(r.calls()), ["mount a", "mount b"]);
        h.send(Msg::Tick(Tick::Health));
        h.send(Msg::Tick(Tick::Fallback));
        let plan = h.reconcile().await;
        assert!(plan.iter().all(|a| a.action == "noop"), "{plan:?}");
        let healthy = |s: &StatusDto| {
            let n = events(s)
                .iter()
                .filter(|e| matches!(e, Event::MountHealthy { .. }))
                .count();
            n == 2
        };
        let s = h.until("both healthy", healthy).await;
        assert!(
            s.mounts.iter().all(|m| m.action == "noop"),
            "{:?}",
            s.mounts
        );
        assert!(r.drv.inspects.load(SeqCst) >= 2);
        assert_eq!(sorted(r.calls()), ["mount a", "mount b"]);
    }

    #[tokio::test]
    async fn warmup_protects_adopted_until_ok() {
        let r = rig("warmup");
        r.set_disc(Err(DiscoveryError::Failed("down".into())));
        let mut h = r.start(r.cfg(RECON, FAKE_ALL), true, &[], &["old"]);
        let s = h
            .until("provider failed, old warming up", |s| {
                s.providers.first().is_some_and(|p| p.last_error.is_some())
                    && mount(s, "old").is_some_and(|m| m.action == "waiting (warming up)")
            })
            .await;
        assert!(!s.ready);
        // A18: an empty Ok is not a report
        r.set_disc(Ok(vec![]));
        h.send(Msg::Api(ApiCmd::Discover));
        let s = h.until("empty ok", |s| s.providers[0].refreshes == 2).await;
        assert!(!s.ready);
        assert_eq!(mount(&s, "old").unwrap().action, "waiting (warming up)");
        assert!(r.calls().is_empty());
        // a non-empty Ok: ready, and the adopted mount nobody wants goes (gracefully)
        r.set_disc(Ok(vec![obs("m1", "10.0.0.9")]));
        h.send(Msg::Api(ApiCmd::Discover));
        h.until("ready, old gone, m1 mounted", |s| {
            s.ready && mount(s, "old").is_none() && is(s, "m1", Availability::Mounted)
        })
        .await;
        assert_eq!(sorted(r.calls()), ["mount m1", "unmount old force=false"]);
    }

    #[tokio::test]
    async fn missing_config_never_unmounts_adopted() {
        let r = rig("missing");
        let grace1 = "offline_grace_period = \"1s\"\nretry_initial = \"1s\"\nretry_max = \"2s\"\n";
        // stands in for the empty default (no file): a 1s grace that must NOT start running while unloaded (A6)
        let mut h = r.start(r.cfg(grace1, ""), false, &[], &["old"]);
        h.until("warming up", |s| {
            mount(s, "old").is_some_and(|m| m.action == "waiting (warming up)")
        })
        .await;
        tokio::time::sleep(Duration::from_millis(1500)).await;
        h.send(Msg::Tick(Tick::Health));
        let plan = h.reconcile().await;
        assert_eq!(plan[0].action, "waiting (warming up)");
        assert!(!h.status.borrow().ready);
        assert!(r.calls().is_empty());
        // the file appears: the grace clock starts now; the provider never reports, so only grace makes it ready
        let loaded = Instant::now();
        assert!(h.reload(r.cfg(grace1, FAKE)).await.ok);
        let s = h
            .until("provider listed", |s| !s.providers.is_empty())
            .await;
        assert!(!s.ready);
        h.until("ready", |s| s.ready).await;
        assert!(loaded.elapsed() >= Duration::from_millis(900));
        h.until("old unmounted", |s| mount(s, "old").is_none())
            .await;
        assert_eq!(r.calls(), ["unmount old force=false"]);
    }

    #[tokio::test]
    async fn child_exit_triggers_probe_and_remount() {
        let r = rig("exit");
        let mut h = r.start(r.cfg(RECON, &machine("a")), true, &[], &[]);
        h.until("mounted", |s| is(s, "a", Availability::Mounted))
            .await;
        let n = r.drv.inspects.load(SeqCst);
        // the process died and took its mount with it; the health ticker is 1h away
        r.drv.fake.mounted.lock().unwrap().remove(&id("a"));
        r.drv.fake.exit("a", "signal: 9 (SIGKILL)");
        let calls = r.clone();
        let s = h
            .until("remounted", move |s| {
                calls.calls().len() == 2 && is(s, "a", Availability::Mounted)
            })
            .await;
        assert!(r.drv.inspects.load(SeqCst) > n);
        let failed = events(&s).into_iter().any(|e| {
            matches!(e, Event::MountFailed { mount, error, attempt: 1, .. }
                if mount == "a" && error == "mount disappeared")
        });
        assert!(failed, "{:?}", s.events);
        assert_eq!(r.calls(), ["mount a", "mount a"]);
    }

    #[tokio::test]
    async fn exit_during_unmount_ignored() {
        let r = rig("exit-unmount");
        let mut h = r.start(r.cfg(RECON, &machine("a")), true, &[], &[]);
        h.until("mounted", |s| is(s, "a", Availability::Mounted))
            .await;
        r.drv.unmount_delay_ms.store(300, SeqCst);
        let n = r.drv.inspects.load(SeqCst);
        assert_eq!(ok(h.unmount("a", false).await), ["a"]);
        h.until("unmounting", |s| is(s, "a", Availability::Unmounting))
            .await;
        r.drv.fake.exit("a", "exited"); // the child exits because of our own unmount
        let s = h
            .until("unmounted and held", |s| {
                mount(s, "a").is_some_and(|m| m.held && m.state == Availability::Eligible)
            })
            .await;
        assert_eq!(r.drv.inspects.load(SeqCst), n);
        assert_eq!(r.calls(), ["mount a", "unmount a force=false"]);
        assert!(
            !events(&s)
                .iter()
                .any(|e| matches!(e, Event::MountFailed { .. }))
        );
    }

    #[tokio::test]
    async fn exit_after_failed_unmount_probed() {
        let r = rig("exit-busy");
        let mut h = r.start(r.cfg(SLOW_RETRY, &machine("a")), true, &[], &[]);
        h.until("mounted", |s| is(s, "a", Availability::Mounted))
            .await;
        r.drv.fake.busy.lock().unwrap().insert(id("a"));
        ok(h.unmount("a", false).await);
        h.until("busy", |s| {
            mount(s, "a").is_some_and(|m| m.last_error.as_deref() == Some(BUSY))
        })
        .await;
        // back to Mounted, a generation past the one on_exit captured; the health ticker is 1h away
        let n = r.drv.inspects.load(SeqCst);
        r.drv.fake.mounted.lock().unwrap().remove(&id("a"));
        r.drv.fake.exit("a", "signal: 9 (SIGKILL)");
        h.until("gone noticed", |s| !is(s, "a", Availability::Mounted))
            .await;
        assert!(r.drv.inspects.load(SeqCst) > n);
    }

    #[tokio::test]
    async fn api_mount_forbidden_for_discover_only() {
        let r = rig("forbidden");
        r.set_disc(Ok(vec![obs("tail-01", "10.0.0.9")]));
        let mut h = r.start(r.cfg(RECON, FAKE), true, &[], &[]);
        h.until("discovered", |s| {
            s.machines
                .iter()
                .any(|m| m.id == "tail-01" && m.verdict == "discover-only")
        })
        .await;
        let e = h.mount("tail-01").await.err().unwrap();
        assert_eq!(
            (e.status, e.error.as_str()),
            (403, "tail-01: discover-only")
        );
        for t in ["ghost", "../x", ""] {
            assert_eq!(h.mount(t).await.err().unwrap().status, 404, "{t}");
        }
        h.reconcile().await;
        assert!(r.calls().is_empty());
        assert!(h.status.borrow().mounts.is_empty());
    }

    #[tokio::test]
    async fn api_unmount_holds_across_restart() {
        let r = rig("hold");
        let cfg = r.cfg(RECON, &machine("a"));
        let mut h = r.start(cfg.clone(), true, &[], &[]);
        h.until("mounted", |s| is(s, "a", Availability::Mounted))
            .await;
        assert_eq!(ok(h.unmount("a", false).await), ["a"]);
        h.until("held and unmounted", |s| {
            mount(s, "a").is_some_and(|m| m.held && !m.desired && m.state == Availability::Eligible)
        })
        .await;
        h.shutdown().await;
        let st = crate::state::read(&r.dir.join("state"));
        assert_eq!(st.held, [id("a")].into());
        assert!(st.mounts.is_empty());

        // restart with a fresh driver and the persisted hold: nothing mounts
        let r2 = Rig {
            drv: Arc::new(TestDriver::new()),
            ..r.clone()
        };
        let held: Vec<&str> = st.held.iter().map(|i| i.as_str()).collect();
        let mut h = r2.start(cfg, true, &held, &[]);
        h.until("probed", |s| s.drivers.iter().any(|d| d.available))
            .await;
        h.reconcile().await;
        assert!(mount(&h.status.borrow(), "a").unwrap().held);
        assert!(r2.calls().is_empty());
        // POST mount clears the hold
        assert_eq!(ok(h.mount("a").await), ["a"]);
        h.until("mounted again", |s| {
            mount(s, "a").is_some_and(|m| !m.held && m.state == Availability::Mounted)
        })
        .await;
        assert_eq!(r2.calls(), ["mount a"]);
        assert!(crate::state::read(&r.dir.join("state")).held.is_empty());
    }

    #[tokio::test]
    async fn api_unmount_busy_stays_degraded() {
        let r = rig("busy");
        let mut h = r.start(r.cfg(RECON, &machine("a")), true, &[], &[]);
        h.until("mounted", |s| is(s, "a", Availability::Mounted))
            .await;
        r.drv.fake.busy.lock().unwrap().insert(id("a"));
        ok(h.unmount("a", false).await);
        let s = h
            .until("busy", |s| {
                mount(s, "a").is_some_and(|m| m.last_error.as_deref() == Some(BUSY))
            })
            .await;
        let m = mount(&s, "a").unwrap();
        assert_eq!(
            (m.state, m.held, m.detail.as_str()),
            (Availability::Mounted, true, BUSY)
        );
        assert!(m.action.starts_with("waiting (backoff"), "{}", m.action);
        let degraded = Event::MountDegraded {
            mount: "a".into(),
            reason: BUSY.into(),
        };
        assert!(events(&s).contains(&degraded));
        // retried gracefully after the backoff, never forced
        h.until("retried", |s| {
            events(s).iter().filter(|e| **e == degraded).count() >= 2
        })
        .await;
        r.drv.fake.busy.lock().unwrap().clear();
        h.until("unmounted", |s| is(s, "a", Availability::Eligible))
            .await;
        let calls = r.calls();
        assert!(
            calls
                .iter()
                .filter(|c| *c == "unmount a force=false")
                .count()
                >= 3
        );
        assert!(!calls.iter().any(|c| c.contains("force=true")), "{calls:?}");
    }

    #[tokio::test]
    async fn reconcile_endpoint_twice_second_all_noop() {
        let r = rig("reconcile");
        r.drv.fake.fail_next("b");
        let mut h = r.start(
            r.cfg(SLOW_RETRY, &(machine("a") + &machine("b"))),
            true,
            &[],
            &[],
        );
        h.until("a mounted, b failed", |s| {
            is(s, "a", Availability::Mounted) && failed(s, "b")
        })
        .await;
        // A11: reconcile does not clear mount_retry_at, so neither call re-arms b
        for _ in 0..2 {
            let plan = h.reconcile().await;
            assert_eq!(plan.len(), 2);
            assert_eq!(plan[0].action, "noop");
            assert!(plan[1].action.starts_with("waiting (backoff"), "{plan:?}");
        }
        assert_eq!(sorted(r.calls()), ["mount a", "mount b"]);
    }

    #[tokio::test]
    async fn events_eligible_and_driver_unavailable_on_transition() {
        let r = rig("events");
        r.set_disc(Ok(vec![obs("m1", "10.0.0.9")]));
        let mut h = r.start(r.cfg(RECON, FAKE), true, &[], &[]);
        h.until("probed and discovered", |s| {
            s.drivers.iter().any(|d| d.available) && s.machines.len() == 1
        })
        .await;
        // B1: DriverUnavailable on Available → Unavailable only
        let probed = |a: DriverAvailability| Msg::Probed([("sshfs".to_string(), a)].into());
        let gone = || DriverAvailability::Unavailable("gone".into());
        let back = || DriverAvailability::Available {
            binary: "/x".into(),
            detail: String::new(),
        };
        h.send(probed(gone()));
        h.send(probed(gone()));
        h.send(probed(back()));
        h.send(probed(gone()));
        let unavailable = |s: &StatusDto| -> Vec<Event> {
            let f = |e: &Event| matches!(e, Event::DriverUnavailable { .. });
            events(s).into_iter().filter(f).collect()
        };
        let s = h
            .until("two transitions", |s| unavailable(s).len() >= 2)
            .await;
        let want = Event::DriverUnavailable {
            driver: "sshfs".into(),
            reason: "gone".into(),
        };
        assert_eq!(unavailable(&s), [want.clone(), want]);
        // B1: MachineEligible when a machine becomes Allowed (here: a reload adds an include filter), once
        let eligible = |s: &StatusDto| -> Vec<Event> {
            let f = |e: &Event| matches!(e, Event::MachineEligible { .. });
            events(s).into_iter().filter(f).collect()
        };
        assert!(eligible(&s).is_empty());
        assert!(h.reload(r.cfg(RECON, FAKE_ALL)).await.ok);
        h.until("allowed", |s| s.machines[0].verdict.starts_with("allowed"))
            .await;
        h.reconcile().await;
        let s = h.status.borrow().clone();
        let want = Event::MachineEligible {
            machine: "m1".into(),
            via: "fake.filter.include".into(),
        };
        assert_eq!(eligible(&s), [want]);
        // ConfigurationReloaded for the reload only, never for the startup apply
        let reloads: Vec<Event> = (events(&s).into_iter())
            .filter(|e| matches!(e, Event::ConfigurationReloaded { .. }))
            .collect();
        let want = Event::ConfigurationReloaded {
            ok: true,
            errors: vec![],
        };
        assert_eq!(reloads, [want]);
    }

    #[tokio::test]
    async fn event_ids_stamped() {
        let r = rig("stamp");
        r.drv.fake.fail_next("a");
        let mut h = r.start(r.cfg(SLOW_RETRY, &machine("a")), true, &[], &[]);
        let s = h.until("failed", |s| failed(s, "a")).await;
        let mut seen = 0;
        for e in events(&s) {
            use Event::*;
            match e {
                MountRequested { mount, .. }
                | MountStarted { mount, .. }
                | MountHealthy { mount }
                | MountDegraded { mount, .. }
                | MountFailed { mount, .. }
                | UnmountStarted { mount, .. }
                | UnmountComplete { mount } => {
                    assert_eq!(mount, "a");
                    seen += 1;
                }
                _ => {}
            }
        }
        assert_eq!(seen, 2, "{:?}", s.events); // MountRequested + MountFailed (core leaves the latter's id empty)
    }

    #[tokio::test]
    async fn executor_panic_recovers() {
        let r = rig("panic");
        r.drv.fake.panic_next("a");
        let mut h = r.start(r.cfg(RECON, &machine("a")), true, &[], &[]);
        // the panic is a failed mount at the executor's generation: Absent with a backoff, not stuck Connecting
        let s = h.until("failed", |s| failed(s, "a")).await;
        let m = mount(&s, "a").unwrap();
        assert_eq!(m.state, Availability::Failed);
        let e = m.last_error.as_deref();
        assert_eq!(e, Some("driver panicked: fake: panic_next"));
        assert!(m.action.starts_with("waiting (backoff"), "{}", m.action);
        h.until("remounted", |s| is(s, "a", Availability::Mounted))
            .await;
        assert_eq!(r.calls(), ["mount a", "mount a"]);
    }

    #[tokio::test]
    async fn drivers_listed_in_first_snapshot() {
        let r = rig("probing");
        let mut h = r.start(r.cfg(RECON, &machine("a")), true, &[], &[]);
        // no await since spawn: on this current-thread runtime the probe task hasn't run yet
        let first: Vec<_> = (h.status.borrow().drivers.iter())
            .map(|d| (d.name.clone(), d.available, d.detail.clone()))
            .collect();
        assert_eq!(first, [("sshfs".to_string(), false, "probing".to_string())]);
        // the desired mount is pending, not Failed: `bifrost mount a` right after start keeps polling
        let s = h.status.borrow().clone();
        let a = mount(&s, "a").unwrap();
        assert_eq!(
            (a.state, a.detail.as_str()),
            (Availability::Eligible, "probing drivers")
        );
        assert_eq!(s.machines[0].state, Availability::Eligible);
        let s = h
            .until("probed", |s| s.drivers.iter().any(|d| d.available))
            .await;
        assert_eq!(s.drivers.len(), 1);
    }

    #[tokio::test]
    async fn offline_grace_unmount_removes_empty_dir() {
        let r = rig("grace-dir");
        let grace1 = "offline_grace_period = \"1s\"\nretry_initial = \"1s\"\nretry_max = \"2s\"\n";
        let mut o = obs("m1", "10.0.0.9");
        o.online = Some(true);
        r.set_disc(Ok(vec![o.clone()]));
        let mut h = r.start(r.cfg(grace1, FAKE_ALL), true, &[], &[]);
        h.until("m1 mounted", |s| is(s, "m1", Availability::Mounted))
            .await;
        let dir = r.root.join("m1");
        std::fs::create_dir_all(&dir).unwrap(); // the fake driver makes none
        // the peer goes offline: Degraded, then row 12 detaches it after the grace period; row 7 keeps it
        // unmounted, so if the peer is then deleted nothing else would ever remove <root>/m1
        o.online = Some(false);
        r.set_disc(Ok(vec![o]));
        h.send(Msg::Api(ApiCmd::Discover));
        r.drv
            .fake
            .set_state("m1", MountState::Degraded("down".into()));
        h.send(Msg::Tick(Tick::Health));
        h.until("m1 offline", |s| is(s, "m1", Availability::Offline))
            .await;
        assert_eq!(r.calls(), ["mount m1", "unmount m1 force=true"]);
        assert!(!dir.exists());
    }

    #[tokio::test]
    async fn next_wakeup_never_spins() {
        let r = rig("spin");
        let mut o = obs("m1", "10.0.0.9");
        o.online = Some(true);
        r.set_disc(Ok(vec![o.clone()]));
        r.drv.fake.fail_next("m1");
        let mut h = r.start(r.cfg(RECON, FAKE_ALL), true, &[], &[]);
        h.until("m1 failed", |s| failed(s, "m1")).await;
        o.online = Some(false);
        r.set_disc(Ok(vec![o]));
        h.send(Msg::Api(ApiCmd::Discover));
        h.until("m1 offline", |s| is(s, "m1", Availability::Offline))
            .await;
        // the ≤1s mount retry passes while row 7 keeps m1 waiting: that past deadline must not make the actor spin
        tokio::time::sleep(Duration::from_millis(1200)).await;
        h.status.borrow_and_update();
        let (mut passes, end) = (0, tokio::time::Instant::now() + Duration::from_secs(1));
        while let Ok(Ok(())) = tokio::time::timeout_at(end, h.status.changed()).await {
            passes += 1;
        }
        assert!(passes <= 2, "{passes} passes in 1s");
        assert_eq!(r.calls(), ["mount m1"]);
    }

    #[tokio::test]
    async fn reload_invalid_keeps_old() {
        let r = rig("reload-invalid");
        let mut h = r.start(r.cfg(RECON, &machine("a")), true, &[], &[]);
        h.until("mounted", |s| is(s, "a", Availability::Mounted))
            .await;
        let bad = ConfigError {
            path: "config.toml:1:1".into(),
            message: "bad".into(),
        };
        let dto = h.config(Err(vec![bad])).await;
        assert!(!dto.ok);
        assert_eq!(dto.errors, ["error: config.toml:1:1: bad"]);
        let s = h
            .until("errors shown", |s| !s.config_errors.is_empty())
            .await;
        assert!(is(&s, "a", Availability::Mounted));
        assert_eq!(s.machines.len(), 1);
        let want = Event::ConfigurationReloaded {
            ok: false,
            errors: dto.errors,
        };
        assert!(events(&s).contains(&want));
        // a valid reload clears the errors; the same spec remounts nothing
        assert!(h.reload(r.cfg(RECON, &machine("a"))).await.ok);
        h.until("errors cleared", |s| s.config_errors.is_empty())
            .await;
        assert_eq!(r.calls(), ["mount a"]);
    }

    #[tokio::test]
    async fn reload_root_change_rejected() {
        let r = rig("reload-root");
        let other = r.dir.join("elsewhere");
        let mut h = r.start(r.cfg(RECON, &machine("a")), true, &[], &[]);
        h.until("mounted", |s| is(s, "a", Availability::Mounted))
            .await;
        let dto = h
            .reload(r.cfg_at(&other, RECON, &(machine("a") + &machine("b"))))
            .await;
        assert!(!dto.ok, "{dto:?}");
        assert!(
            dto.errors[0].contains("mount.root change requires restart"),
            "{dto:?}"
        );
        let s = h
            .until("error shown", |s| !s.config_errors.is_empty())
            .await;
        assert!(s.machines.iter().all(|m| m.id != "b"));
        assert_eq!(s.mount_root, r.root.display().to_string());
        h.shutdown().await;

        // missing-config start: the default root is active, so a first file with another root is rejected (A22)
        let r = rig("reload-root-missing");
        let mut h = r.start(r.cfg(RECON, ""), false, &[], &[]);
        let dto = h.reload(r.cfg_at(&other, RECON, &machine("a"))).await;
        assert!(!dto.ok, "{dto:?}");
        let s = h
            .until("error shown", |s| !s.config_errors.is_empty())
            .await;
        assert!(!s.ready && s.machines.is_empty());
    }

    #[tokio::test]
    async fn reload_changed_provider_respawns_keeps_observations() {
        let r = rig("respawn");
        r.set_disc(Ok(vec![obs("m1", "10.0.0.9")]));
        let mut h = r.start(r.cfg(RECON, FAKE), true, &[], &[]);
        h.until("m1", |s| s.machines.len() == 1).await;
        assert_eq!(r.builds.load(SeqCst), 1);
        // unchanged: the task is kept
        assert!(h.reload(r.cfg(RECON, FAKE)).await.ok);
        assert_eq!(r.builds.load(SeqCst), 1);
        // changed: aborted and respawned; its first refresh fails, and m1 is kept (frozen), not dropped
        r.set_disc(Err(DiscoveryError::Failed("down".into())));
        assert!(
            h.reload(r.cfg(RECON, &format!("{FAKE}interval = \"2h\"\n")))
                .await
                .ok
        );
        assert_eq!(r.builds.load(SeqCst), 2);
        let s = h
            .until("respawned", |s| s.providers[0].last_error.is_some())
            .await;
        assert_eq!((s.machines.len(), s.providers[0].machines), (1, 1));
    }

    #[tokio::test]
    async fn reload_retries_failed_provider_build() {
        let r = rig("rebuild");
        r.fail_build.store(true, SeqCst);
        r.set_disc(Ok(vec![obs("m1", "10.0.0.9")]));
        let mut h = r.start(r.cfg(RECON, FAKE), true, &[], &[]);
        h.until("build failed", |s| s.providers[0].last_error.is_some())
            .await;
        // B11: no task; an unchanged reload (SIGHUP, `bifrost config reload`) retries the build
        r.fail_build.store(false, SeqCst);
        assert!(h.reload(r.cfg(RECON, FAKE)).await.ok);
        h.until("m1", |s| s.machines.len() == 1).await;
        assert_eq!(r.builds.load(SeqCst), 2);
    }

    #[tokio::test]
    async fn reload_unbuildable_provider_freezes_observations() {
        let r = rig("rebuild-freeze");
        r.set_disc(Ok(vec![obs("m1", "10.0.0.9")]));
        // below config's 1s floor, so the 3 x interval expiry comes within the test
        let every = |ms| {
            let mut c = r.cfg(RECON, FAKE_ALL);
            c.providers[0].interval = Duration::from_millis(ms);
            c
        };
        let mut h = r.start(every(100), true, &[], &[]);
        h.until("m1 mounted", |s| is(s, "m1", Availability::Mounted))
            .await;
        // a changed provider whose build fails (a URL config check accepts but reqwest can't parse): no task,
        // so no refresh ever marks it failing; its view must freeze like a failing refresh's, not age out
        r.fail_build.store(true, SeqCst);
        assert!(h.reload(every(101)).await.ok);
        h.until("build failed", |s| s.providers[0].last_error.is_some())
            .await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        h.reconcile().await; // expire() runs only in a pass
        let s = h.status.borrow().clone();
        assert_eq!(s.machines.len(), 1, "{:?}", s.machines);
        assert!(is(&s, "m1", Availability::Mounted));
        assert_eq!(r.calls(), ["mount m1"]);
    }

    #[tokio::test]
    async fn fallback_tick_retries_failed_provider_build() {
        let r = rig("rebuild-tick");
        r.fail_build.store(true, SeqCst);
        r.set_disc(Ok(vec![obs("m1", "10.0.0.9")]));
        let mut h = r.start(r.cfg(RECON, FAKE), true, &[], &[]);
        h.until("build failed", |s| s.providers[0].last_error.is_some())
            .await;
        // nobody reloads: the fallback tick (every reconcile_interval) retries the build on its own
        r.fail_build.store(false, SeqCst);
        h.send(Msg::Tick(Tick::Fallback));
        h.until("m1", |s| s.machines.len() == 1).await;
        assert_eq!(r.builds.load(SeqCst), 2);
    }

    #[tokio::test]
    async fn stale_task_gen_dropped() {
        let r = rig("taskgen");
        r.set_disc(Ok(vec![obs("m1", "10.0.0.9")]));
        let mut h = r.start(r.cfg(RECON, FAKE), true, &[], &[]);
        h.until("m1", |s| s.machines.len() == 1).await;
        // respawn: the provider's task_gen goes 1 → 2
        assert!(
            h.reload(r.cfg(RECON, &format!("{FAKE}interval = \"2h\"\n")))
                .await
                .ok
        );
        let late = |provider: &str, task_gen, id: &str| Msg::Discovery {
            provider: provider.into(),
            task_gen,
            result: Ok(vec![obs(id, "10.0.0.8")]),
        };
        h.send(late("fake", 1, "m2")); // the aborted task's result
        h.send(late("gone", 2, "m3")); // a removed provider's
        h.reconcile().await;
        assert!(h.status.borrow().machines.iter().all(|m| m.id == "m1"));
        h.send(late("fake", 2, "m2"));
        h.until("current task_gen accepted", |s| s.machines.len() == 2)
            .await;
    }
}
