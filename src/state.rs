use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;

use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sysinfo::{Pid, ProcessesToUpdate, Signal, System};

use crate::storage::PunchDirs;
use crate::types::{TunnelRecord, TunnelStatus};

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct PersistedState {
    pub recent_domain: Option<String>,
    #[serde(default)]
    pub tunnels: BTreeMap<String, TunnelRecord>,
}

#[derive(Debug, Clone)]
pub struct StateStore {
    dirs: PunchDirs,
}

impl StateStore {
    pub fn new(dirs: PunchDirs) -> Self {
        Self { dirs }
    }

    pub fn load(&self) -> Result<PersistedState> {
        match fs::read_to_string(self.dirs.state_file()) {
            Ok(contents) => serde_json::from_str(&contents).context("解析状态文件失败"),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(PersistedState::default()),
            Err(error) => Err(error).context("读取状态文件失败"),
        }
    }

    pub fn save(&self, state: &PersistedState) -> Result<()> {
        self.dirs.ensure()?;
        let payload = serde_json::to_string_pretty(state)?;
        fs::write(self.dirs.state_file(), payload).context("写入状态文件失败")
    }

    pub fn upsert(&self, record: TunnelRecord) -> Result<()> {
        let mut state = self.load()?;
        state.recent_domain = Some(record.domain.clone());
        state.tunnels.insert(record.domain.clone(), record);
        self.save(&state)
    }

    pub fn remove(&self, domain: &str) -> Result<Option<TunnelRecord>> {
        let mut state = self.load()?;
        let removed = state.tunnels.remove(domain);
        if state.recent_domain.as_deref() == Some(domain) {
            state.recent_domain = state.tunnels.keys().next_back().cloned();
        }
        self.save(&state)?;
        Ok(removed)
    }

    pub fn find(&self, domain: &str) -> Result<Option<TunnelRecord>> {
        Ok(self.load()?.tunnels.get(domain).cloned())
    }

    pub fn recent(&self) -> Result<Option<TunnelRecord>> {
        let state = self.load()?;
        Ok(state
            .recent_domain
            .as_ref()
            .and_then(|domain| state.tunnels.get(domain))
            .cloned())
    }

    pub fn list(&self) -> Result<Vec<TunnelRecord>> {
        Ok(self.load()?.tunnels.into_values().collect())
    }

    pub fn refresh_statuses(&self) -> Result<Vec<TunnelRecord>> {
        let mut state = self.load()?;
        let mut system = System::new_all();
        system.refresh_processes(ProcessesToUpdate::All, true);

        for record in state.tunnels.values_mut() {
            record.last_seen_at = Some(Utc::now());
            record.status = match record.pid {
                Some(pid) if system.process(Pid::from_u32(pid)).is_some() => TunnelStatus::Running,
                Some(_) => TunnelStatus::Stopped,
                None => record.status,
            };
        }

        let records = state.tunnels.values().cloned().collect::<Vec<_>>();
        self.save(&state)?;
        Ok(records)
    }

    pub fn stop_process(&self, domain: &str) -> Result<Option<TunnelRecord>> {
        let mut state = self.load()?;
        let mut system = System::new_all();
        system.refresh_processes(ProcessesToUpdate::All, true);

        let record = match state.tunnels.get_mut(domain) {
            Some(record) => record,
            None => return Ok(None),
        };

        if let Some(pid) = record.pid {
            if let Some(process) = system.process(Pid::from_u32(pid)) {
                let _ = process
                    .kill_with(Signal::Term)
                    .unwrap_or_else(|| process.kill());
            }
        }

        record.pid = None;
        record.status = TunnelStatus::Stopped;
        record.last_seen_at = Some(Utc::now());

        let cloned = record.clone();
        self.save(&state)?;
        Ok(Some(cloned))
    }
}
