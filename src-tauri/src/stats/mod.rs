//! RAM/CPU sampler (PLAN.md 3.4). Every 1.5 s: for each agent, sum Private
//! Bytes and working set over its Job Object's PIDs and take CPU from the
//! job's accounting; for Spectacli itself, walk its own process tree (WebView2,
//! the ConPTY hosts) minus the agent trees. One `stats:tick` carries it all.

mod proc;

use crate::pty::PtyManager;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

pub const INTERVAL: Duration = Duration::from_millis(1500);

/// One process tree's usage. Bytes are exact; `cpu` is percent of the whole
/// machine (0–100), 0 on the first sample of a tree.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    /// Private Bytes (what Process Explorer shows): the "RAM" figure.
    pub private_bytes: u64,
    /// Working set, for the tooltip.
    pub working_set: u64,
    pub cpu: f32,
    pub processes: u32,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatsTick {
    /// Keyed by agent (PTY) id. Agents whose process has exited are left out.
    pub agents: HashMap<String, Usage>,
    /// Spectacli's own process, its WebView2 processes, and the ConPTY
    /// hosts: everything under the app that is not inside an agent's job.
    pub spectacli: Usage,
}

/// CPU percent of the whole machine for `cpu` (100 ns units) used over `wall`.
fn cpu_percent(cpu: u64, wall: Duration, cores: u32) -> f32 {
    let wall = wall.as_nanos() as f64 / 100.0;
    if wall <= 0.0 || cores == 0 {
        return 0.0;
    }
    (cpu as f64 / wall / cores as f64 * 100.0).clamp(0.0, 100.0) as f32
}

/// State carried between samples to turn CPU totals into rates.
pub struct Sampler {
    cores: u32,
    last: Option<Instant>,
    /// Job CPU total per agent at the last sample.
    agent_cpu: HashMap<String, u64>,
    /// CPU total per Spectacli process at the last sample, keyed by
    /// (pid, creation time) so a reused PID never counts as the old process.
    self_cpu: HashMap<(u32, u64), u64>,
}

impl Sampler {
    pub fn new() -> Self {
        Self { cores: proc::cores(), last: None, agent_cpu: HashMap::new(), self_cpu: HashMap::new() }
    }

    pub fn sample(&mut self, jobs: &[(String, std::sync::Arc<crate::pty::job::Job>)]) -> StatsTick {
        let now = Instant::now();
        let wall = self.last.map(|t| now - t);
        self.last = Some(now);
        let rate = |delta: u64| wall.map_or(0.0, |w| cpu_percent(delta, w, self.cores));

        let mut tick = StatsTick::default();
        let mut agent_pids = HashSet::new();
        let mut agent_cpu = HashMap::new();
        for (id, job) in jobs {
            let pids = job.pids().unwrap_or_default();
            if pids.is_empty() {
                continue;
            }
            let mut u = Usage::default();
            for &pid in &pids {
                if let Some(p) = proc::read(pid) {
                    u.private_bytes += p.private_bytes;
                    u.working_set += p.working_set;
                    u.processes += 1;
                }
            }
            agent_pids.extend(pids);
            if let Ok(cpu) = job.cpu_time() {
                // Job accounting keeps exited children's time, so this total
                // only grows; a missing previous value means a new agent.
                if let Some(&prev) = self.agent_cpu.get(id) {
                    u.cpu = rate(cpu.saturating_sub(prev));
                }
                agent_cpu.insert(id.clone(), cpu);
            }
            tick.agents.insert(id.clone(), u);
        }
        self.agent_cpu = agent_cpu;

        let mut self_cpu = HashMap::new();
        let mut cpu_delta = 0;
        for p in proc::own_tree(&agent_pids) {
            tick.spectacli.private_bytes += p.private_bytes;
            tick.spectacli.working_set += p.working_set;
            tick.spectacli.processes += 1;
            let key = (p.pid, p.created);
            // After the first sample, an unseen process started within the
            // interval, so all of its CPU time belongs to it.
            match self.self_cpu.get(&key) {
                Some(&prev) => cpu_delta += p.cpu.saturating_sub(prev),
                None if wall.is_some() => cpu_delta += p.cpu,
                None => {}
            }
            self_cpu.insert(key, p.cpu);
        }
        tick.spectacli.cpu = rate(cpu_delta);
        self.self_cpu = self_cpu;
        tick
    }
}

/// Start the sampler thread. It runs for the life of the app.
pub fn spawn(app: AppHandle) {
    let _ = std::thread::Builder::new().name("stats".into()).spawn(move || {
        let mut sampler = Sampler::new();
        loop {
            let jobs = app.state::<PtyManager>().jobs();
            let tick = sampler.sample(&jobs);
            drop(jobs);
            let _ = app.emit("stats:tick", tick);
            std::thread::sleep(INTERVAL);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_percent_scales_by_wall_and_cores() {
        // 0.75 s of CPU over 1.5 s on 4 cores = 12.5% of the machine.
        assert_eq!(cpu_percent(7_500_000, Duration::from_millis(1500), 4), 12.5);
        assert_eq!(cpu_percent(1, Duration::ZERO, 4), 0.0);
        assert_eq!(cpu_percent(u64::MAX, Duration::from_secs(1), 1), 100.0);
    }

    #[cfg(windows)]
    #[test]
    fn measures_agent_and_self() {
        use crate::pty::job::Job;
        use std::os::windows::io::AsRawHandle;
        use std::sync::Arc;

        // A busy child in a job: CPU must show up after the second sample.
        let mut child = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-Command", "$x=0; while ($true) { $x++ }"])
            .spawn()
            .unwrap();
        let job = Arc::new(Job::new().unwrap());
        job.assign(child.as_raw_handle()).unwrap();
        let jobs = vec![("a".to_string(), job.clone())];

        let mut s = Sampler::new();
        let first = s.sample(&jobs);
        std::thread::sleep(Duration::from_millis(1000));
        let second = s.sample(&jobs);
        let _ = job.terminate();
        let _ = child.wait();

        let a = second.agents["a"];
        assert_eq!(first.agents["a"].cpu, 0.0);
        assert!(a.processes >= 1);
        assert!(a.private_bytes > 1 << 20, "{a:?}");
        assert!(a.working_set > 1 << 20, "{a:?}");
        assert!(a.cpu > 100.0 / s.cores as f32 * 0.5, "busy loop should use most of a core: {a:?}");

        // The test process itself; the agent's PID is excluded from it.
        let me = second.spectacli;
        assert!(me.processes >= 1 && me.private_bytes > 0, "{me:?}");
        assert!(!proc::own_tree(&HashSet::new()).is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn exited_agents_are_dropped() {
        use crate::pty::job::Job;
        use std::sync::Arc;
        let jobs = vec![("gone".to_string(), Arc::new(Job::new().unwrap()))];
        assert!(Sampler::new().sample(&jobs).agents.is_empty());
    }
}
