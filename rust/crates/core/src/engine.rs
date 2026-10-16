use crate::{
    controller::{Controller, CountryProbe, HttpCountryProbe, Mihomo},
    gate::Gate,
    nodes::{CheckError, Snapshot},
    proxy::Proxy,
};
use anyhow::{bail, Result};
use chrono::Utc;
use guard_protocol::{log, Settings, State};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;

struct Inner {
    state: State,
    settings: Settings,
    snapshot: Option<Snapshot>,
    epoch: u64,
    verified: Option<Instant>,
    health: Option<Instant>,
    next_probe: Instant,
    probe_epoch: Option<u64>,
    switching: bool,
}
pub struct Engine {
    inner: Mutex<Inner>,
    pub gate: Arc<Gate>,
    pub stop: CancellationToken,
    controller: Arc<dyn Controller>,
    probe: Arc<dyn CountryProbe>,
    switch_lock: tokio::sync::Mutex<()>,
    persist: bool,
}
impl Engine {
    pub fn new(settings: Settings) -> Result<Arc<Self>> {
        let controller = Arc::new(Mihomo::new(&settings.mihomo_pipe_name)?);
        Ok(Self::with_services(
            settings,
            controller,
            Arc::new(HttpCountryProbe),
            true,
        ))
    }
    pub fn with_services(
        settings: Settings,
        controller: Arc<dyn Controller>,
        probe: Arc<dyn CountryProbe>,
        persist: bool,
    ) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(Inner {
                state: State {
                    proxy_port: settings.proxy_port,
                    ..State::default()
                },
                settings,
                snapshot: None,
                epoch: 0,
                verified: None,
                health: None,
                next_probe: Instant::now(),
                probe_epoch: None,
                switching: false,
            }),
            gate: Arc::new(Gate::default()),
            stop: CancellationToken::new(),
            controller,
            probe,
            switch_lock: tokio::sync::Mutex::new(()),
            persist,
        })
    }
    pub fn state(&self) -> State {
        let mut state = self.inner.lock().unwrap().state.clone();
        // Admission never relies on wall-clock JSON fields.
        if state.allowed && self.gate.permit().is_none() {
            state.allowed = false;
            state.status = "blocked".into();
            state.reason = "正在重新检查连接".into();
        }
        state
    }
    pub fn settings(&self) -> Settings {
        self.inner.lock().unwrap().settings.clone()
    }
    pub fn update_settings(&self, mutate: impl FnOnce(&mut Settings)) -> Result<()> {
        let mut i = self.inner.lock().unwrap();
        let mut settings = i.settings.clone();
        mutate(&mut settings);
        if self.persist {
            settings.save()?;
        }
        i.settings = settings;
        Ok(())
    }
    pub fn start(self: &Arc<Self>) -> Result<()> {
        let port = self.settings().proxy_port;
        let proxy = match Proxy::bind(port, self.gate.clone(), self.stop.clone()) {
            Ok(proxy) => proxy,
            Err(error) => {
                let mut i = self.inner.lock().unwrap();
                self.invalidate_locked(
                    &mut i,
                    "portConflict",
                    "本地端口已被占用，请关闭占用程序后重新启动",
                    "blocked",
                    "recheck",
                );
                return Err(error);
            }
        };
        tokio::spawn(proxy.run());
        let engine = self.clone();
        tokio::spawn(async move {
            engine.monitor().await;
        });
        let weak = Arc::downgrade(self);
        std::thread::Builder::new()
            .name("guard-watchdog".into())
            .spawn(move || loop {
                std::thread::sleep(Duration::from_millis(100));
                let Some(engine) = weak.upgrade() else {
                    break;
                };
                if engine.stop.is_cancelled() {
                    break;
                }
                if engine.gate.expire() {
                    // Close sockets before acquiring the state/settings lock: a stalled
                    // disk write or UI reader must never delay revocation.
                    let mut i = engine.inner.lock().unwrap();
                    engine.invalidate_locked(
                        &mut i,
                        "expired",
                        "连接检查已过期",
                        "blocked",
                        "recheck",
                    );
                }
            })?;
        Ok(())
    }
    pub fn invalidate(&self) {
        let mut i = self.inner.lock().unwrap();
        self.invalidate_locked(&mut i, "checking", "正在检查连接", "checking", "recheck");
    }
    pub fn shutdown(&self) {
        self.stop.cancel();
        let mut i = self.inner.lock().unwrap();
        self.invalidate_locked(&mut i, "stopped", "守卫已退出", "blocked", "recheck");
    }
    fn invalidate_locked(
        &self,
        i: &mut Inner,
        code: &str,
        reason: &str,
        status: &str,
        action: &str,
    ) {
        self.gate.revoke();
        i.epoch += 1;
        i.snapshot = None;
        i.verified = None;
        i.health = None;
        i.next_probe = Instant::now();
        i.probe_epoch = None;
        i.state.status = status.into();
        i.state.reason = reason.into();
        i.state.reason_code = code.into();
        i.state.suggested_action = action.into();
        i.state.allowed = false;
        i.state.verified_at = None;
        i.state.country.clear();
    }
    pub async fn switch_preferred(&self, node: Option<String>) -> Result<()> {
        let _switch = self.switch_lock.lock().await;
        let node = {
            let mut i = self.inner.lock().unwrap();
            let node = node
                .or_else(|| i.settings.preferred_node.clone())
                .filter(|s| !s.is_empty());
            let Some(node) = node else {
                bail!("请先在 Clash 中选择可用节点");
            };
            i.switching = true;
            self.invalidate_locked(&mut i, "switching", "正在恢复代理", "checking", "recheck");
            node
        };
        let result = self.controller.select(&node).await;
        let mut i = self.inner.lock().unwrap();
        i.switching = false;
        if result.is_ok() {
            self.invalidate_locked(&mut i, "checking", "正在检查连接", "checking", "recheck");
        } else {
            self.invalidate_locked(
                &mut i,
                "restoreFailed",
                "代理恢复失败，请打开 Clash",
                "blocked",
                "openClash",
            );
        }
        result
    }
    async fn monitor(self: Arc<Self>) {
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {_=self.stop.cancelled()=>break,_=interval.tick()=>{}}
            let epoch = {
                let i = self.inner.lock().unwrap();
                if i.switching {
                    continue;
                }
                i.epoch
            };
            let result = tokio::select! {_=self.stop.cancelled()=>break,v=tokio::time::timeout(Duration::from_millis(2500),self.controller.read())=>v};
            let mut i = self.inner.lock().unwrap();
            if epoch != i.epoch || i.switching || self.stop.is_cancelled() {
                continue;
            }
            let snapshot = match result {
                Ok(Ok(snapshot)) if snapshot.port != i.settings.proxy_port => snapshot,
                error => {
                    let (code, reason) = match &error {
                        Ok(Err(e)) => e
                            .downcast_ref::<CheckError>()
                            .map(|e| (e.code, e.message))
                            .unwrap_or(("controllerFailed", "暂时无法读取 Clash 状态")),
                        Ok(Ok(_)) => ("proxyPort", "Clash 端口与守卫端口冲突"),
                        Err(_) => ("controllerTimeout", "Clash 响应超时"),
                    };
                    let action = if code == "clashUnavailable" {
                        "startClash"
                    } else if i.settings.preferred_node.is_some()
                        && matches!(code, "mode" | "nodeUnclear")
                    {
                        "restoreProxy"
                    } else if matches!(code, "controllerTimeout" | "controllerFailed") {
                        "recheck"
                    } else {
                        "openClash"
                    };
                    self.invalidate_locked(&mut i, code, reason, "blocked", action);
                    i.state.last_check_at = Some(Utc::now());
                    continue;
                }
            };
            if i.snapshot
                .as_ref()
                .is_none_or(|s| s.fingerprint != snapshot.fingerprint)
            {
                self.invalidate_locked(&mut i, "checking", "正在检查连接", "checking", "recheck");
                i.snapshot = Some(snapshot.clone());
            }
            i.health = Some(Instant::now());
            i.state.mode = snapshot.mode.clone();
            i.state.node = snapshot.node.clone();
            i.state.node_path = snapshot.path.clone();
            i.state.upstream_port = snapshot.port;
            i.state.last_check_at = Some(Utc::now());
            self.refresh(&mut i);
            if i.probe_epoch.is_none() && i.next_probe <= Instant::now() {
                let epoch = i.epoch;
                i.probe_epoch = Some(epoch);
                i.next_probe = Instant::now() + Duration::from_secs(30);
                let engine = self.clone();
                tokio::spawn(async move {
                    engine.verify(snapshot, epoch).await;
                });
            }
        }
        self.gate.revoke();
    }
    fn refresh(&self, i: &mut Inner) {
        let (Some(verified), Some(health), Some(snapshot)) =
            (i.verified, i.health, i.snapshot.as_ref())
        else {
            return;
        };
        let lease = remaining_lease(verified.elapsed(), health.elapsed());
        if lease.is_zero() {
            self.invalidate_locked(i, "expired", "连接检查已过期", "blocked", "recheck");
            return;
        }
        if !self.gate.allow(snapshot.port, lease) {
            self.invalidate_locked(i, "expired", "连接检查已过期", "blocked", "recheck");
            return;
        }
        i.state.status = "allowed".into();
        i.state.allowed = true;
        i.state.reason = "检查通过".into();
        i.state.reason_code = "ready".into();
        i.state.suggested_action.clear();
    }
    async fn verify(self: Arc<Self>, snapshot: Snapshot, epoch: u64) {
        let result =
            tokio::time::timeout(Duration::from_secs(9), self.probe.check(snapshot.port)).await;
        let mut i = self.inner.lock().unwrap();
        if self.stop.is_cancelled()
            || i.epoch != epoch
            || i.snapshot
                .as_ref()
                .is_none_or(|s| s.fingerprint != snapshot.fingerprint)
        {
            return;
        }
        i.probe_epoch = None;
        match result {
            Ok(Ok(country)) if country == "US" => {
                i.verified = Some(Instant::now());
                i.state.country = country;
                i.state.verified_at = Some(Utc::now());
                self.refresh(&mut i);
                if i.state.allowed
                    && i.settings
                        .preferred_node
                        .as_ref()
                        .is_none_or(|s| s.is_empty())
                {
                    i.settings.preferred_node = Some(snapshot.restorable);
                    if self.persist && i.settings.save().is_err() {
                        log("无法保存恢复节点");
                    }
                }
            }
            other => {
                self.gate.revoke();
                i.verified = None;
                i.state.verified_at = None;
                i.state.allowed = false;
                i.state.status = "blocked".into();
                if let Ok(Ok(country)) = other {
                    i.state.country = country;
                    i.state.reason = "当前连接不符合要求，请切换代理".into();
                    i.state.reason_code = "country".into();
                    i.state.suggested_action = if i.settings.preferred_node.is_some() {
                        "restoreProxy"
                    } else {
                        "openClash"
                    }
                    .into();
                } else {
                    i.state.country.clear();
                    i.state.reason = "暂时无法验证连接".into();
                    i.state.reason_code = "probeFailed".into();
                    i.state.suggested_action = "recheck".into();
                }
            }
        }
    }
}
pub fn remaining_lease(country: Duration, health: Duration) -> Duration {
    Duration::from_secs(45)
        .saturating_sub(country)
        .min(Duration::from_secs(3).saturating_sub(health))
}
