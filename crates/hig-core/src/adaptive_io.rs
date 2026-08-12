use crate::{AdaptiveIoReport, AdaptiveIoStageReport, AdaptiveIoTransitionReport};
use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

const WINDOW_MIN_SAMPLES: usize = 8;
const WINDOW_MIN_BYTES: u64 = 8 * 1024 * 1024;
const WINDOW_FORCE_BYTES: u64 = 32 * 1024 * 1024;
const WINDOW_MAX_SAMPLES: usize = 64;
const SLOW_THROUGHPUT_BYTES_PER_SEC: f64 = 48.0 * 1024.0 * 1024.0;
const RECOVERY_THROUGHPUT_BYTES_PER_SEC: f64 = 96.0 * 1024.0 * 1024.0;
const RELATIVE_SLOWDOWN_RATIO: f64 = 0.50;
const RELATIVE_RECOVERY_RATIO: f64 = 0.80;
const SMALL_IO_MAX_BYTES: u64 = 256 * 1024;
const SMALL_IO_SLOW_P95_US: u64 = 20_000;
const SMALL_IO_RECOVERY_P95_US: u64 = 8_000;
const REQUIRED_BAD_WINDOWS: u32 = 2;
const REQUIRED_GOOD_WINDOWS: u32 = 2;
const TRANSITION_COOLDOWN: Duration = Duration::from_millis(750);
const MAX_TRANSITION_EVENTS: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum IoDirection {
    Read,
    Write,
}

impl IoDirection {
    fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Sample {
    bytes: u64,
    elapsed_us: u64,
}

#[derive(Debug, Default)]
struct DirectionState {
    samples: VecDeque<Sample>,
    bytes: u64,
    elapsed_us: u64,
    best_throughput: f64,
    bad_windows: u32,
    good_windows: u32,
}

#[derive(Debug, Default)]
struct StageState {
    samples: u64,
    bytes: u64,
    io_us: u64,
    wait_us: u64,
}

#[derive(Debug)]
struct ControllerState {
    target: usize,
    in_flight: usize,
    min_observed: usize,
    max_observed: usize,
    transitions: u64,
    constrained_entries: u64,
    recovery_steps: u64,
    started: Instant,
    state_started: Instant,
    normal_us: u64,
    constrained_us: u64,
    constraint_direction: Option<IoDirection>,
    constraint_stage: Option<&'static str>,
    adaptation: BTreeMap<(&'static str, IoDirection), DirectionState>,
    stages: BTreeMap<&'static str, StageState>,
    transition_events: Vec<AdaptiveIoTransitionReport>,
    last_transition: Instant,
}

#[derive(Debug)]
pub(crate) struct AdaptiveIoController {
    min_concurrency: usize,
    max_concurrency: usize,
    transition_cooldown: Duration,
    state: Mutex<ControllerState>,
    available: Condvar,
}

pub(crate) struct IoPermit {
    controller: Arc<AdaptiveIoController>,
    stage: &'static str,
    direction: IoDirection,
    bytes: u64,
    started: Instant,
    completed: bool,
}

impl AdaptiveIoController {
    pub(crate) fn new(max_concurrency: usize) -> Arc<Self> {
        Self::new_with_cooldown(max_concurrency, TRANSITION_COOLDOWN)
    }

    fn new_with_cooldown(max_concurrency: usize, transition_cooldown: Duration) -> Arc<Self> {
        let max_concurrency = max_concurrency.max(1);
        let now = Instant::now();
        Arc::new(Self {
            min_concurrency: 1,
            max_concurrency,
            transition_cooldown,
            state: Mutex::new(ControllerState {
                target: max_concurrency,
                in_flight: 0,
                min_observed: max_concurrency,
                max_observed: max_concurrency,
                transitions: 0,
                constrained_entries: 0,
                recovery_steps: 0,
                started: now,
                state_started: now,
                normal_us: 0,
                constrained_us: 0,
                constraint_direction: None,
                constraint_stage: None,
                adaptation: BTreeMap::new(),
                stages: BTreeMap::new(),
                transition_events: Vec::new(),
                last_transition: now.checked_sub(transition_cooldown).unwrap_or(now),
            }),
            available: Condvar::new(),
        })
    }

    pub(crate) fn acquire(
        self: &Arc<Self>,
        stage: &'static str,
        direction: IoDirection,
        bytes: u64,
    ) -> IoPermit {
        let wait_started = Instant::now();
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        while state.in_flight >= state.target {
            state = self
                .available
                .wait(state)
                .unwrap_or_else(|error| error.into_inner());
        }
        state.in_flight += 1;
        let wait_us = wait_started.elapsed().as_micros() as u64;
        let stage_state = state.stages.entry(stage).or_default();
        stage_state.wait_us = stage_state.wait_us.saturating_add(wait_us);
        drop(state);
        IoPermit {
            controller: self.clone(),
            stage,
            direction,
            bytes,
            started: Instant::now(),
            completed: false,
        }
    }

    pub(crate) fn target_concurrency(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .target
    }

    pub(crate) fn max_concurrency(&self) -> usize {
        self.max_concurrency
    }

    #[cfg(test)]
    pub(crate) fn constrained(&self) -> bool {
        self.target_concurrency() < self.max_concurrency
    }

    pub(crate) fn report(&self) -> AdaptiveIoReport {
        let state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        let now = Instant::now();
        let mut normal_us = state.normal_us;
        let mut constrained_us = state.constrained_us;
        let elapsed = now.duration_since(state.state_started).as_micros() as u64;
        if state.target == self.max_concurrency {
            normal_us = normal_us.saturating_add(elapsed);
        } else {
            constrained_us = constrained_us.saturating_add(elapsed);
        }
        AdaptiveIoReport {
            enabled: true,
            initial_concurrency: self.max_concurrency,
            min_concurrency: self.min_concurrency,
            max_concurrency: self.max_concurrency,
            final_concurrency: state.target,
            min_observed_concurrency: state.min_observed,
            max_observed_concurrency: state.max_observed,
            transitions: state.transitions,
            constrained_entries: state.constrained_entries,
            recovery_steps: state.recovery_steps,
            normal_us,
            constrained_us,
            total_us: now.duration_since(state.started).as_micros() as u64,
            final_constraint_stage: state.constraint_stage.map(str::to_string),
            final_constraint_direction: state
                .constraint_direction
                .map(IoDirection::as_str)
                .map(str::to_string),
            stages: state
                .stages
                .iter()
                .map(|(name, stage)| {
                    (
                        (*name).to_string(),
                        AdaptiveIoStageReport {
                            samples: stage.samples,
                            bytes: stage.bytes,
                            io_us: stage.io_us,
                            wait_us: stage.wait_us,
                        },
                    )
                })
                .collect(),
            transition_events: state.transition_events.clone(),
        }
    }

    #[cfg(test)]
    fn observe_for_test(
        &self,
        stage: &'static str,
        direction: IoDirection,
        bytes: u64,
        elapsed: Duration,
    ) {
        self.complete(stage, direction, bytes, elapsed, false);
    }

    fn complete(
        &self,
        stage: &'static str,
        direction: IoDirection,
        bytes: u64,
        elapsed: Duration,
        release_permit: bool,
    ) {
        let elapsed_us = (elapsed.as_micros() as u64).max(1);
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        let stage_state = state.stages.entry(stage).or_default();
        stage_state.samples = stage_state.samples.saturating_add(1);
        stage_state.bytes = stage_state.bytes.saturating_add(bytes);
        stage_state.io_us = stage_state.io_us.saturating_add(elapsed_us);

        let constrained = state.target < self.max_concurrency;
        let direction_state = state.adaptation.entry((stage, direction)).or_default();
        direction_state
            .samples
            .push_back(Sample { bytes, elapsed_us });
        direction_state.bytes = direction_state.bytes.saturating_add(bytes);
        direction_state.elapsed_us = direction_state.elapsed_us.saturating_add(elapsed_us);
        while direction_state.samples.len() > WINDOW_MAX_SAMPLES {
            if let Some(sample) = direction_state.samples.pop_front() {
                direction_state.bytes = direction_state.bytes.saturating_sub(sample.bytes);
                direction_state.elapsed_us =
                    direction_state.elapsed_us.saturating_sub(sample.elapsed_us);
            }
        }

        if window_ready(direction_state) {
            let evaluation = evaluate_window(direction_state, constrained);
            let bad_windows = direction_state.bad_windows;
            let good_windows = direction_state.good_windows;
            reset_window(direction_state);
            self.apply_decision(
                &mut state,
                stage,
                direction,
                evaluation,
                bad_windows,
                good_windows,
            );
        }
        if release_permit {
            state.in_flight = state.in_flight.saturating_sub(1);
        }
        drop(state);
        if release_permit {
            self.available.notify_all();
        }
    }

    fn apply_decision(
        &self,
        state: &mut ControllerState,
        stage: &'static str,
        direction: IoDirection,
        evaluation: WindowEvaluation,
        bad_windows: u32,
        good_windows: u32,
    ) {
        match evaluation.decision {
            WindowDecision::Degrade => {
                if bad_windows < REQUIRED_BAD_WINDOWS {
                    return;
                }
                if state.last_transition.elapsed() < self.transition_cooldown {
                    return;
                }
                if let Some(adaptation) = state.adaptation.get_mut(&(stage, direction)) {
                    adaptation.bad_windows = 0;
                }
                let next = (state.target / 2).max(self.min_concurrency);
                if next < state.target {
                    let previous = state.target;
                    self.account_state_time(state);
                    if state.target == self.max_concurrency {
                        state.constrained_entries += 1;
                    }
                    state.target = next;
                    state.min_observed = state.min_observed.min(next);
                    state.transitions += 1;
                    state.last_transition = Instant::now();
                    push_transition(state, stage, direction, previous, next, evaluation);
                }
                state.constraint_direction = Some(direction);
                state.constraint_stage = Some(stage);
            }
            WindowDecision::Recover => {
                if state.constraint_direction != Some(direction) {
                    return;
                }
                if state.target == self.max_concurrency || good_windows < REQUIRED_GOOD_WINDOWS {
                    return;
                }
                if state.last_transition.elapsed() < self.transition_cooldown {
                    return;
                }
                if let Some(adaptation) = state.adaptation.get_mut(&(stage, direction)) {
                    adaptation.good_windows = 0;
                }
                let increase = (state.target / 4).max(1);
                let next = state
                    .target
                    .saturating_add(increase)
                    .min(self.max_concurrency);
                if next > state.target {
                    let previous = state.target;
                    self.account_state_time(state);
                    state.target = next;
                    state.max_observed = state.max_observed.max(next);
                    state.transitions += 1;
                    state.recovery_steps += 1;
                    state.last_transition = Instant::now();
                    push_transition(state, stage, direction, previous, next, evaluation);
                    if next == self.max_concurrency {
                        state.constraint_direction = None;
                        state.constraint_stage = None;
                    }
                    self.available.notify_all();
                }
            }
            WindowDecision::Stable => {}
        }
    }

    fn account_state_time(&self, state: &mut ControllerState) {
        let now = Instant::now();
        let elapsed = now.duration_since(state.state_started).as_micros() as u64;
        if state.target == self.max_concurrency {
            state.normal_us = state.normal_us.saturating_add(elapsed);
        } else {
            state.constrained_us = state.constrained_us.saturating_add(elapsed);
        }
        state.state_started = now;
    }
}

impl IoPermit {
    pub(crate) fn finish(mut self) {
        self.finish_inner();
    }

    pub(crate) fn finish_with_bytes(mut self, bytes: u64) {
        self.bytes = bytes;
        self.finish_inner();
    }

    fn finish_inner(&mut self) {
        if self.completed {
            return;
        }
        self.completed = true;
        let elapsed = self.started.elapsed();
        self.controller
            .complete(self.stage, self.direction, self.bytes, elapsed, true);
    }
}

impl Drop for IoPermit {
    fn drop(&mut self) {
        self.finish_inner();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WindowDecision {
    Degrade,
    Recover,
    Stable,
}

#[derive(Debug, Clone, Copy)]
struct WindowEvaluation {
    decision: WindowDecision,
    reason: &'static str,
    throughput_bytes_per_sec: f64,
    small_io_p95_us: u64,
}

fn window_ready(state: &DirectionState) -> bool {
    (state.samples.len() >= WINDOW_MIN_SAMPLES && state.bytes >= WINDOW_MIN_BYTES)
        || state.bytes >= WINDOW_FORCE_BYTES
}

fn evaluate_window(state: &mut DirectionState, constrained: bool) -> WindowEvaluation {
    let throughput = if state.elapsed_us == 0 {
        f64::INFINITY
    } else {
        state.bytes as f64 * 1_000_000.0 / state.elapsed_us as f64
    };
    let previous_best = state.best_throughput;
    state.best_throughput = throughput.max(state.best_throughput * 0.98);
    let mut small_latencies = state
        .samples
        .iter()
        .filter(|sample| sample.bytes <= SMALL_IO_MAX_BYTES)
        .map(|sample| sample.elapsed_us)
        .collect::<Vec<_>>();
    small_latencies.sort_unstable();
    let small_p95 = percentile_95(&small_latencies);
    let enough_small_samples = small_latencies.len() >= WINDOW_MIN_SAMPLES;
    let absolute_slow =
        state.bytes >= WINDOW_MIN_BYTES && throughput < SLOW_THROUGHPUT_BYTES_PER_SEC;
    let relative_slow = previous_best > 0.0
        && throughput < previous_best * RELATIVE_SLOWDOWN_RATIO
        && throughput < RECOVERY_THROUGHPUT_BYTES_PER_SEC;
    let latency_slow = enough_small_samples && small_p95 >= SMALL_IO_SLOW_P95_US;
    let recovered_throughput = throughput >= RECOVERY_THROUGHPUT_BYTES_PER_SEC
        || (state.best_throughput > 0.0
            && throughput >= state.best_throughput * RELATIVE_RECOVERY_RATIO);
    let recovered_latency = !enough_small_samples || small_p95 <= SMALL_IO_RECOVERY_P95_US;

    if latency_slow {
        state.bad_windows = state.bad_windows.saturating_add(1);
        state.good_windows = 0;
        WindowEvaluation {
            decision: WindowDecision::Degrade,
            reason: "small-io-latency",
            throughput_bytes_per_sec: throughput,
            small_io_p95_us: small_p95,
        }
    } else if absolute_slow || relative_slow {
        state.bad_windows = state.bad_windows.saturating_add(1);
        state.good_windows = 0;
        WindowEvaluation {
            decision: WindowDecision::Degrade,
            reason: if absolute_slow {
                "absolute-throughput"
            } else {
                "relative-throughput"
            },
            throughput_bytes_per_sec: throughput,
            small_io_p95_us: small_p95,
        }
    } else if constrained && recovered_throughput && recovered_latency {
        state.good_windows = state.good_windows.saturating_add(1);
        state.bad_windows = 0;
        WindowEvaluation {
            decision: WindowDecision::Recover,
            reason: "sustained-recovery",
            throughput_bytes_per_sec: throughput,
            small_io_p95_us: small_p95,
        }
    } else {
        state.bad_windows = 0;
        state.good_windows = 0;
        WindowEvaluation {
            decision: WindowDecision::Stable,
            reason: "stable",
            throughput_bytes_per_sec: throughput,
            small_io_p95_us: small_p95,
        }
    }
}

fn push_transition(
    state: &mut ControllerState,
    stage: &'static str,
    direction: IoDirection,
    from_concurrency: usize,
    to_concurrency: usize,
    evaluation: WindowEvaluation,
) {
    if state.transition_events.len() == MAX_TRANSITION_EVENTS {
        state.transition_events.remove(0);
    }
    state.transition_events.push(AdaptiveIoTransitionReport {
        at_us: state.started.elapsed().as_micros() as u64,
        stage: stage.to_string(),
        direction: direction.as_str().to_string(),
        reason: evaluation.reason.to_string(),
        from_concurrency,
        to_concurrency,
        throughput_mib_s: evaluation.throughput_bytes_per_sec / 1024.0 / 1024.0,
        small_io_p95_us: evaluation.small_io_p95_us,
    });
}

fn reset_window(state: &mut DirectionState) {
    state.samples.clear();
    state.bytes = 0;
    state.elapsed_us = 0;
}

fn percentile_95(values: &[u64]) -> u64 {
    if values.is_empty() {
        return 0;
    }
    let index = ((values.len() - 1) * 95) / 100;
    values[index]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_controller(max_concurrency: usize) -> Arc<AdaptiveIoController> {
        AdaptiveIoController::new_with_cooldown(max_concurrency, Duration::ZERO)
    }

    fn feed_window(
        controller: &AdaptiveIoController,
        bytes: u64,
        elapsed: Duration,
        windows: usize,
    ) {
        for _ in 0..windows {
            for _ in 0..8 {
                controller.observe_for_test("test", IoDirection::Read, bytes, elapsed);
            }
        }
    }

    #[test]
    fn sustained_slow_io_reduces_concurrency() {
        let controller = test_controller(8);
        feed_window(&controller, 1024 * 1024, Duration::from_millis(40), 2);
        assert_eq!(controller.target_concurrency(), 4);
        assert!(controller.constrained());
    }

    #[test]
    fn one_slow_window_does_not_change_concurrency() {
        let controller = test_controller(8);
        feed_window(&controller, 1024 * 1024, Duration::from_millis(40), 1);
        assert_eq!(controller.target_concurrency(), 8);
    }

    #[test]
    fn constrained_controller_recovers_after_sustained_fast_io() {
        let controller = test_controller(8);
        feed_window(&controller, 1024 * 1024, Duration::from_millis(40), 2);
        assert_eq!(controller.target_concurrency(), 4);
        feed_window(&controller, 1024 * 1024, Duration::from_millis(2), 2);
        assert_eq!(controller.target_concurrency(), 5);
        assert_eq!(controller.report().recovery_steps, 1);
    }

    #[test]
    fn fast_io_keeps_full_concurrency() {
        let controller = test_controller(8);
        feed_window(&controller, 1024 * 1024, Duration::from_millis(2), 6);
        assert_eq!(controller.target_concurrency(), 8);
        assert_eq!(controller.report().transitions, 0);
    }

    #[test]
    fn fast_writes_do_not_clear_slow_read_hysteresis() {
        let controller = test_controller(8);
        feed_window(&controller, 1024 * 1024, Duration::from_millis(40), 1);
        for _ in 0..8 {
            controller.observe_for_test(
                "write",
                IoDirection::Write,
                1024 * 1024,
                Duration::from_millis(2),
            );
        }
        feed_window(&controller, 1024 * 1024, Duration::from_millis(40), 1);
        assert_eq!(controller.target_concurrency(), 4);
    }

    #[test]
    fn a_new_stage_learns_its_own_baseline() {
        let controller = test_controller(8);
        for _ in 0..16 {
            controller.observe_for_test(
                "cache-write",
                IoDirection::Write,
                1024 * 1024,
                Duration::from_millis(2),
            );
        }
        for _ in 0..16 {
            controller.observe_for_test(
                "archive-write",
                IoDirection::Write,
                1024 * 1024,
                Duration::from_millis(14),
            );
        }
        assert_eq!(controller.target_concurrency(), 8);
        assert_eq!(controller.report().transitions, 0);
    }

    #[test]
    fn high_absolute_throughput_is_not_a_relative_slowdown() {
        let controller = test_controller(8);
        feed_window(&controller, 1024 * 1024, Duration::from_millis(1), 2);
        feed_window(&controller, 1024 * 1024, Duration::from_millis(4), 2);
        assert_eq!(controller.target_concurrency(), 8);
        assert_eq!(controller.report().transitions, 0);
    }

    #[test]
    fn transition_cooldown_suppresses_immediate_oscillation() {
        let controller = AdaptiveIoController::new_with_cooldown(4, Duration::from_millis(40));
        feed_window(&controller, 1024 * 1024, Duration::from_millis(40), 2);
        assert_eq!(controller.target_concurrency(), 2);
        feed_window(&controller, 1024 * 1024, Duration::from_millis(40), 2);
        assert_eq!(controller.target_concurrency(), 2);
        std::thread::sleep(Duration::from_millis(50));
        feed_window(&controller, 1024 * 1024, Duration::from_millis(40), 1);
        assert_eq!(controller.target_concurrency(), 1);
    }

    #[test]
    fn recovery_requires_improvement_in_the_constrained_direction() {
        let controller = test_controller(8);
        feed_window(&controller, 1024 * 1024, Duration::from_millis(40), 2);
        for _ in 0..16 {
            controller.observe_for_test(
                "write",
                IoDirection::Write,
                1024 * 1024,
                Duration::from_millis(2),
            );
        }
        assert_eq!(controller.target_concurrency(), 4);
        feed_window(&controller, 1024 * 1024, Duration::from_millis(2), 2);
        assert_eq!(controller.target_concurrency(), 5);
    }

    #[test]
    fn permits_follow_a_reduced_target_without_cancelling_in_flight_io() {
        let controller = test_controller(2);
        feed_window(&controller, 1024 * 1024, Duration::from_millis(40), 2);
        assert_eq!(controller.target_concurrency(), 1);

        let first = controller.acquire("test", IoDirection::Read, 1);
        let (tx, rx) = std::sync::mpsc::channel();
        let worker_controller = controller.clone();
        let worker = std::thread::spawn(move || {
            let second = worker_controller.acquire("test", IoDirection::Read, 1);
            tx.send(()).unwrap();
            second.finish_with_bytes(1);
        });
        assert!(rx.recv_timeout(Duration::from_millis(50)).is_err());
        first.finish_with_bytes(1);
        rx.recv_timeout(Duration::from_secs(1)).unwrap();
        worker.join().unwrap();
    }

    #[test]
    fn report_tracks_stage_io() {
        let controller = test_controller(4);
        controller.observe_for_test(
            "scan-read",
            IoDirection::Read,
            4096,
            Duration::from_millis(1),
        );
        let report = controller.report();
        let stage = report.stages.get("scan-read").unwrap();
        assert_eq!(stage.samples, 1);
        assert_eq!(stage.bytes, 4096);
        assert_eq!(stage.io_us, 1000);
    }
}
