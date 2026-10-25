use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::Error;
use crate::json::Value;

const MAX_RECORDS: usize = 128;
const MAX_ACTIVE: usize = 64;

#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Label {
    Local,
    Preparation,
    Coder,
    Reviewer,
    Cleanup,
    Model,
    GitValidate,
    GitIndex,
    GitSubject,
    GitHead,
    GitBranch,
    GitOther,
    OwnedGit,
    OwnedNode,
    OwnedCargo,
    CoderCurrent,
    ResultCurrent,
    ReviewerCurrent,
    ReviewEvidence,
    ReviewPrepare,
    E2EPrepare,
    E2ECurrent,
    SourceCapture,
    SourceAudit,
    Finalization,
    FullVerification,
    Classification,
    ArchivePrepare,
    ArchiveAuthority,
    ArchiveExecute,
    ArchivePostcheck,
    Transition,
    ProofStampCapture,
    ProofStampCurrent,
    ProofOwnerCheck,
    ProofSource,
    ProofCompare,
    ProofGitHead,
    ProofSignatureHead,
    ProofGitIndex,
    ProofGitRefs,
    ProofGitConfig,
    ProofGitTree,
    ProofGitBlob,
    ProofGitBlobBatch,
    ProofBlobRequested,
    ProofGitClone,
    ProofGitOther,
    ProofGitExecute,
    ProofCommandExecute,
    ProofImagePlan,
    ProofHostImage,
    ClassPrepare,
    ClassCandidate,
    ClassBaseline,
    ScratchCreate,
    ScratchBase,
    ScratchClose,
    ConsumerSetup,
    ConsumerRun,
    NodeVersion,
    CliCommand,
    StartGitPaths,
    StartGitRoot,
    StartGitCommon,
    StartGitDirectory,
    StartGitRevision,
    StartGitBranch,
    StartGitConfig,
    StartGitStatus,
    StartGitWorktrees,
    StartGitSourceDiff,
    StartGitUntracked,
    StartGitOther,
    StartGitExecute,
    StartGitIgnore,
    NoInputOwned,
    NoInputDirect,
    NoInputDispatch,
    NoInputPreflight,
    NoInputSpawn,
    NoInputJobCreate,
    NoInputWrappedSpawn,
    NoInputWait,
    NoInputLeaderWindow,
    NoInputGroupWindow,
    NoInputTryWait,
    NoInputJobQuery,
    NoInputMemberOpen,
    NoInputMemberWait,
    NoInputLeaderObserved,
    NoInputJobZero,
    NoInputMembersDone,
    NoInputStdoutDrain,
    NoInputStdoutDispatch,
    NoInputStdoutFirstByte,
    NoInputStdoutPoll,
    NoInputStdoutPending,
    NoInputStderrDrain,
    NoInputStderrDispatch,
    NoInputStderrFirstByte,
    NoInputStderrPoll,
    NoInputStderrPending,
    NoInputCleanup,
    StartGitImage,
}

impl Label {
    fn name(self) -> &'static str {
        match self {
            Self::Local => "delivery.local",
            Self::Preparation => "delivery.preparation",
            Self::Coder => "seat.coder.await",
            Self::Reviewer => "seat.reviewer.await",
            Self::Cleanup => "ownership.close",
            Self::Model => "model.await",
            Self::GitValidate => "git.validate",
            Self::GitIndex => "git.index.check",
            Self::GitSubject => "git.head-branch",
            Self::GitHead => "git.head",
            Self::GitBranch => "git.branch",
            Self::GitOther => "git.other",
            Self::OwnedGit => "owned.git.await",
            Self::OwnedNode => "owned.node.await",
            Self::OwnedCargo => "owned.cargo.await",
            Self::CoderCurrent => "subject.coder.current",
            Self::ResultCurrent => "result.private.current",
            Self::ReviewerCurrent => "subject.reviewer.current",
            Self::ReviewEvidence => "subject.review.evidence",
            Self::ReviewPrepare => "review.host.prepare",
            Self::E2EPrepare => "review.host.observations",
            Self::E2ECurrent => "subject.e2e.current",
            Self::SourceCapture => "source.capture",
            Self::SourceAudit => "source.audit",
            Self::Finalization => "finalization.await",
            Self::FullVerification => "verification.full",
            Self::Classification => "verification.classification",
            Self::ArchivePrepare => "archive.prepare",
            Self::ArchiveAuthority => "archive.authority",
            Self::ArchiveExecute => "archive.execute",
            Self::ArchivePostcheck => "archive.postcheck",
            Self::Transition => "context.transition",
            Self::ProofStampCapture => "proof.stamp.capture",
            Self::ProofStampCurrent => "proof.stamp.current",
            Self::ProofOwnerCheck => "proof.owner.check",
            Self::ProofSource => "proof.source.capture",
            Self::ProofCompare => "proof.subject.compare",
            Self::ProofGitHead => "proof.git.head",
            Self::ProofSignatureHead => "proof.head.from-refs",
            Self::ProofGitIndex => "proof.git.index",
            Self::ProofGitRefs => "proof.git.refs",
            Self::ProofGitConfig => "proof.git.config",
            Self::ProofGitTree => "proof.git.tree",
            Self::ProofGitBlob => "proof.git.blob",
            Self::ProofGitBlobBatch => "proof.git.blob.batch",
            Self::ProofBlobRequested => "proof.blob.requested",
            Self::ProofGitClone => "proof.git.clone",
            Self::ProofGitOther => "proof.git.other",
            Self::ProofGitExecute => "proof.git.execute.await",
            Self::ProofCommandExecute => "proof.command.execute.await",
            Self::ProofImagePlan => "proof.image.plan",
            Self::ProofHostImage => "proof.host-image.current",
            Self::ClassPrepare => "classify.proof.prepare",
            Self::ClassCandidate => "classify.candidate",
            Self::ClassBaseline => "classify.baseline",
            Self::ScratchCreate => "proof.scratch.create",
            Self::ScratchBase => "proof.scratch.base",
            Self::ScratchClose => "proof.scratch.close",
            Self::ConsumerSetup => "proof.consumer.setup",
            Self::ConsumerRun => "proof.consumer.test",
            Self::NodeVersion => "proof.consumer.node-version",
            Self::CliCommand => "proof.cli.command",
            Self::StartGitPaths => "start.git.paths",
            Self::StartGitRoot => "start.git.root",
            Self::StartGitCommon => "start.git.common",
            Self::StartGitDirectory => "start.git.directory",
            Self::StartGitRevision => "start.git.revision",
            Self::StartGitBranch => "start.git.branch",
            Self::StartGitConfig => "start.git.configuration",
            Self::StartGitStatus => "start.git.status",
            Self::StartGitWorktrees => "start.git.worktrees",
            Self::StartGitSourceDiff => "start.git.source-diff",
            Self::StartGitUntracked => "start.git.untracked",
            Self::StartGitOther => "start.git.other",
            Self::StartGitExecute => "start.git.execute.await",
            Self::StartGitIgnore => "start.git.ignore",
            Self::NoInputOwned => "owned.noinput.execute",
            Self::NoInputDirect => "direct.noinput.execute",
            Self::NoInputDispatch => "noinput.caller-dispatch",
            Self::NoInputPreflight => "noinput.preflight",
            Self::NoInputSpawn => "noinput.spawn.sync",
            Self::NoInputJobCreate => "noinput.spawn.job-create",
            Self::NoInputWrappedSpawn => "noinput.spawn.wrapped",
            Self::NoInputWait => "noinput.wait",
            Self::NoInputLeaderWindow => "noinput.leader-observation-window",
            Self::NoInputGroupWindow => "noinput.job-observation-window",
            Self::NoInputTryWait => "noinput.leader-try-wait.sync",
            Self::NoInputJobQuery => "noinput.job-query.sync",
            Self::NoInputMemberOpen => "noinput.member-open.sync",
            Self::NoInputMemberWait => "noinput.member-wait.sync",
            Self::NoInputLeaderObserved => "noinput.leader-observed",
            Self::NoInputJobZero => "noinput.job-zero-observed",
            Self::NoInputMembersDone => "noinput.members-settled-observed",
            Self::NoInputStdoutDrain => "noinput.stdout.drain",
            Self::NoInputStdoutDispatch => "noinput.stdout.caller-dispatch",
            Self::NoInputStdoutFirstByte => "noinput.stdout.first-byte-observed",
            Self::NoInputStdoutPoll => "noinput.stdout.poll.sync",
            Self::NoInputStdoutPending => "noinput.stdout.pending-to-repoll",
            Self::NoInputStderrDrain => "noinput.stderr.drain",
            Self::NoInputStderrDispatch => "noinput.stderr.caller-dispatch",
            Self::NoInputStderrFirstByte => "noinput.stderr.first-byte-observed",
            Self::NoInputStderrPoll => "noinput.stderr.poll.sync",
            Self::NoInputStderrPending => "noinput.stderr.pending-to-repoll",
            Self::NoInputCleanup => "noinput.cleanup",
            Self::StartGitImage => "start.git.image-current",
        }
    }
}

#[derive(Clone)]
pub struct Timings(Arc<Mutex<State>>);

#[derive(Clone)]
struct Record {
    id: u32,
    parent: Option<u32>,
    label: Label,
    started: Duration,
    ended: Option<Duration>,
    state: &'static str,
    error: Option<(&'static str, Option<&'static str>)>,
    seat: &'static str,
    context: u32,
    children: Vec<(Duration, Duration)>,
    children_truncated: bool,
    exclusive: Option<Duration>,
}

#[derive(Clone, Default)]
struct Aggregate {
    count: u32,
    total: Duration,
    maximum: Duration,
    dropped: u32,
    exclusive_total: Duration,
    exclusive_known: u32,
    exclusive_unknown: u32,
}

struct State {
    epoch: Instant,
    next: u32,
    active: Vec<Record>,
    records: VecDeque<Record>,
    aggregates: BTreeMap<Label, Aggregate>,
    dropped: u32,
    seat: &'static str,
    context: u32,
}

pub(crate) struct Span {
    timings: Timings,
    id: Option<u32>,
}

fn micros(value: Duration) -> Value {
    value
        .as_micros()
        .to_string()
        .parse::<f64>()
        .map_or(Value::Null, Value::Number)
}

fn record_value(record: &Record) -> Value {
    Value::object(vec![
        ("id", Value::Number(f64::from(record.id))),
        (
            "parent",
            record
                .parent
                .map_or(Value::Null, |value| Value::Number(f64::from(value))),
        ),
        ("label", Value::string(record.label.name())),
        ("start_us", micros(record.started)),
        ("end_us", record.ended.map_or(Value::Null, micros)),
        ("state", Value::string(record.state)),
        ("seat", Value::string(record.seat)),
        ("context", Value::Number(f64::from(record.context))),
        ("exclusive_us", record.exclusive.map_or(Value::Null, micros)),
        (
            "child_intervals_truncated",
            Value::Boolean(record.children_truncated),
        ),
        (
            "error_name",
            record
                .error
                .map_or(Value::Null, |error| Value::string(error.0)),
        ),
        (
            "error_code",
            record
                .error
                .and_then(|error| error.1)
                .map_or(Value::Null, Value::string),
        ),
    ])
}

impl Default for Timings {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(State {
            epoch: Instant::now(),
            next: 0,
            active: Vec::new(),
            records: VecDeque::new(),
            aggregates: BTreeMap::new(),
            dropped: 0,
            seat: "host",
            context: 0,
        })))
    }
}

impl Timings {
    pub(crate) fn seat(&self, seat: &'static str, context: u32) {
        if let Ok(mut state) = self.0.lock() {
            state.seat = seat;
            state.context = context;
        }
    }

    pub(crate) fn start(&self, label: Label) -> Span {
        self.start_parented(label, None, false)
    }

    fn start_parented(&self, label: Label, parent: Option<u32>, explicit: bool) -> Span {
        let id = match self.0.lock() {
            Ok(mut state) => {
                if state.active.len() >= MAX_ACTIVE || state.next == u32::MAX {
                    state.dropped = state.dropped.saturating_add(1);
                    None
                } else {
                    state.next += 1;
                    let parent = if explicit {
                        parent
                    } else {
                        state.active.last().map(|record| record.id)
                    };
                    let missing_parent =
                        explicit && !state.active.iter().any(|record| Some(record.id) == parent);
                    let record = Record {
                        id: state.next,
                        parent,
                        label,
                        started: state.epoch.elapsed(),
                        ended: None,
                        state: "pending",
                        error: None,
                        seat: state.seat,
                        context: state.context,
                        children: Vec::new(),
                        children_truncated: missing_parent,
                        exclusive: None,
                    };
                    let id = record.id;
                    state.active.push(record);
                    Some(id)
                }
            }
            Err(_) => None,
        };
        Span {
            timings: self.clone(),
            id,
        }
    }

    fn end(&self, id: u32, status: &'static str, error: Option<&Error>) {
        if let Ok(mut state) = self.0.lock() {
            if let Some(index) = state.active.iter().position(|record| record.id == id) {
                let mut record = state.active.remove(index);
                let ended = state.epoch.elapsed();
                let duration = ended.saturating_sub(record.started);
                record.ended = Some(ended);
                record.state = status;
                record.error = error.map(|error| (error.name, error.code));
                if !record.children_truncated
                    && !state.active.iter().any(|child| child.parent == Some(id))
                {
                    record.exclusive = Some(exclusive(record.started, ended, &record.children));
                }
                if let Some(parent) = state
                    .active
                    .iter_mut()
                    .find(|parent| Some(parent.id) == record.parent)
                {
                    if parent.children.len() < MAX_RECORDS {
                        parent.children.push((record.started, ended));
                    } else {
                        parent.children_truncated = true;
                    }
                }
                let aggregate = state.aggregates.entry(record.label).or_default();
                aggregate.count = aggregate.count.saturating_add(1);
                aggregate.total = aggregate.total.saturating_add(duration);
                aggregate.maximum = aggregate.maximum.max(duration);
                if let Some(exclusive) = record.exclusive {
                    aggregate.exclusive_total = aggregate.exclusive_total.saturating_add(exclusive);
                    aggregate.exclusive_known = aggregate.exclusive_known.saturating_add(1);
                } else {
                    aggregate.exclusive_unknown = aggregate.exclusive_unknown.saturating_add(1);
                }
                if status == "scope-exit" {
                    aggregate.dropped = aggregate.dropped.saturating_add(1);
                }
                state.records.push_back(record);
                if state.records.len() > MAX_RECORDS {
                    state.records.pop_front();
                    state.dropped = state.dropped.saturating_add(1);
                }
            }
        }
    }

    pub fn snapshot(&self) -> Value {
        let copied = self.0.lock().map(|state| {
            (
                state.epoch.elapsed(),
                state.active.clone(),
                state.records.clone(),
                state.aggregates.clone(),
                state.dropped,
            )
        });
        match copied {
            Err(_) => Value::object(vec![("unavailable", Value::Boolean(true))]),
            Ok((elapsed, active, records, aggregates, dropped)) => Value::object(vec![
                ("clock", Value::string("native-ownership-std-instant")),
                ("elapsed_us", micros(elapsed)),
                (
                    "active",
                    Value::Array(
                        active
                            .iter()
                            .map(|record| {
                                let mut current = record.clone();
                                if !record.children_truncated {
                                    let mut children = record.children.clone();
                                    children.extend(
                                        active
                                            .iter()
                                            .filter(|child| child.parent == Some(record.id))
                                            .map(|child| (child.started, elapsed)),
                                    );
                                    current.exclusive =
                                        Some(exclusive(record.started, elapsed, &children));
                                }
                                record_value(&current)
                            })
                            .collect(),
                    ),
                ),
                (
                    "records",
                    Value::Array(records.iter().map(record_value).collect()),
                ),
                ("dropped_records", Value::Number(f64::from(dropped))),
                (
                    "aggregates",
                    Value::Array(
                        aggregates
                            .iter()
                            .map(|(label, aggregate)| {
                                Value::object(vec![
                                    ("label", Value::string(label.name())),
                                    ("count", Value::Number(f64::from(aggregate.count))),
                                    ("total_us", micros(aggregate.total)),
                                    ("max_us", micros(aggregate.maximum)),
                                    ("exclusive_total_us", micros(aggregate.exclusive_total)),
                                    (
                                        "exclusive_known_count",
                                        Value::Number(f64::from(aggregate.exclusive_known)),
                                    ),
                                    (
                                        "exclusive_unknown_count",
                                        Value::Number(f64::from(aggregate.exclusive_unknown)),
                                    ),
                                    (
                                        "unclassified_exits",
                                        Value::Number(f64::from(aggregate.dropped)),
                                    ),
                                ])
                            })
                            .collect(),
                    ),
                ),
            ]),
        }
    }
}

fn exclusive(started: Duration, ended: Duration, children: &[(Duration, Duration)]) -> Duration {
    let mut children = children.to_vec();
    children.sort_unstable();
    let mut cursor = started;
    let mut covered = Duration::ZERO;
    for (start, end) in children {
        let start = start.max(started).max(cursor);
        let end = end.min(ended);
        if end > start {
            covered = covered.saturating_add(end.saturating_sub(start));
        }
        cursor = cursor.max(end);
    }
    ended.saturating_sub(started).saturating_sub(covered)
}

impl Span {
    pub(crate) fn child(&self, label: Label) -> Span {
        match self.id {
            Some(id) => self.timings.start_parented(label, Some(id), true),
            None => Span {
                timings: self.timings.clone(),
                id: None,
            },
        }
    }

    pub(crate) fn finish_status(mut self, success: bool) {
        if let Some(id) = self.id.take() {
            self.timings
                .end(id, if success { "ok" } else { "error" }, None);
        }
    }

    pub(crate) fn finish(mut self, error: Option<&Error>) {
        if let Some(id) = self.id.take() {
            self.timings
                .end(id, if error.is_some() { "error" } else { "ok" }, error);
        }
    }
}

impl Drop for Span {
    fn drop(&mut self) {
        if let Some(id) = self.id.take() {
            self.timings.end(id, "scope-exit", None);
        }
    }
}

pub(crate) fn finish(span: Option<Span>, error: Option<&Error>) {
    if let Some(span) = span {
        span.finish(error);
    }
}

#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) struct ProcessTrace {
    root: Span,
    leader: std::sync::atomic::AtomicBool,
    job_zero: std::sync::atomic::AtomicBool,
    members: std::sync::atomic::AtomicBool,
}

#[cfg_attr(not(windows), allow(dead_code))]
impl ProcessTrace {
    pub(crate) fn new(parent: &Span, label: Label) -> Arc<Self> {
        Arc::new(Self {
            root: parent.child(label),
            leader: std::sync::atomic::AtomicBool::new(false),
            job_zero: std::sync::atomic::AtomicBool::new(false),
            members: std::sync::atomic::AtomicBool::new(false),
        })
    }

    pub(crate) fn scope(&self, label: Label) -> Span {
        self.root.child(label)
    }

    pub(crate) fn leader_seen(&self) -> bool {
        self.leader.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub(crate) fn leader(&self) {
        self.once(&self.leader, Label::NoInputLeaderObserved);
    }
    pub(crate) fn job_zero(&self) {
        self.once(&self.job_zero, Label::NoInputJobZero);
    }
    pub(crate) fn members(&self) {
        self.once(&self.members, Label::NoInputMembersDone);
    }

    fn once(&self, observed: &std::sync::atomic::AtomicBool, label: Label) {
        if !observed.swap(true, std::sync::atomic::Ordering::Relaxed) {
            self.scope(label).finish(None);
        }
    }
}

pub(crate) struct ReadTrace<R> {
    inner: R,
    drain: Option<Span>,
    dispatch: Option<Span>,
    pending: Option<Span>,
    first: bool,
    stdout: bool,
}

impl<R> ReadTrace<R> {
    pub(crate) fn new(inner: R, trace: Option<&Arc<ProcessTrace>>, stdout: bool) -> Self {
        let drain = trace.map(|trace| {
            trace.scope(if stdout {
                Label::NoInputStdoutDrain
            } else {
                Label::NoInputStderrDrain
            })
        });
        let dispatch = drain.as_ref().map(|span| {
            span.child(if stdout {
                Label::NoInputStdoutDispatch
            } else {
                Label::NoInputStderrDispatch
            })
        });
        Self {
            inner,
            drain,
            dispatch,
            pending: None,
            first: false,
            stdout,
        }
    }
}

impl<R: tokio::io::AsyncRead + Unpin> tokio::io::AsyncRead for ReadTrace<R> {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
        buffer: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let this = self.get_mut();
        finish(this.dispatch.take(), None);
        finish(this.pending.take(), None);
        let before = buffer.filled().len();
        let remaining = buffer.remaining();
        let poll = this.drain.as_ref().map(|span| {
            span.child(if this.stdout {
                Label::NoInputStdoutPoll
            } else {
                Label::NoInputStderrPoll
            })
        });
        let observed = std::pin::Pin::new(&mut this.inner).poll_read(context, buffer);
        if let Some(poll) = poll {
            poll.finish_status(!matches!(&observed, std::task::Poll::Ready(Err(_))));
        }
        match &observed {
            std::task::Poll::Pending => {
                this.pending = this.drain.as_ref().map(|span| {
                    span.child(if this.stdout {
                        Label::NoInputStdoutPending
                    } else {
                        Label::NoInputStderrPending
                    })
                });
            }
            std::task::Poll::Ready(Ok(())) => {
                let bytes = buffer.filled().len().saturating_sub(before);
                if bytes != 0 && !this.first {
                    this.first = true;
                    if let Some(drain) = &this.drain {
                        drain
                            .child(if this.stdout {
                                Label::NoInputStdoutFirstByte
                            } else {
                                Label::NoInputStderrFirstByte
                            })
                            .finish(None);
                    }
                }
                if bytes == 0 && remaining != 0 {
                    if let Some(drain) = this.drain.take() {
                        drain.finish(None);
                    }
                }
            }
            std::task::Poll::Ready(Err(_)) => {
                if let Some(drain) = this.drain.take() {
                    drain.finish_status(false);
                }
            }
        }
        observed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passive_explicit_sibling_spans_keep_parallel_parentage_and_union() {
        let timings = Timings::default();
        let root = timings.start(Label::Local);
        let first = root.child(Label::NoInputStdoutDrain);
        let second = root.child(Label::NoInputStderrDrain);
        let pending = timings.snapshot();
        let active = entries(&pending, "active");
        let id = active.first().and_then(|record| record.get("id"));
        assert!(
            active
                .iter()
                .skip(1)
                .all(|record| record.get("parent") == id)
        );
        first.finish(None);
        second.finish(None);
        root.finish(None);
        let done = timings.snapshot();
        assert!(entries(&done, "active").is_empty());
        assert!(
            entries(&done, "records")
                .iter()
                .all(|record| record.get("exclusive_us") != Some(&Value::Null))
        );
    }

    fn entries<'a>(value: &'a Value, key: &str) -> &'a [Value] {
        match value.get(key) {
            Some(Value::Array(items)) => items,
            _ => &[],
        }
    }

    #[test]
    fn passive_timing_keeps_active_parent_and_settled_intervals() -> Result<(), Error> {
        let timings = Timings::default();
        let parent = timings.start(Label::Local);
        let child = timings.start(Label::GitValidate);
        let active = timings.snapshot();
        let pending = entries(&active, "active");
        assert_eq!(pending.len(), 2);
        assert_eq!(
            pending.get(1).and_then(|record| record.get("parent")),
            pending.first().and_then(|record| record.get("id"))
        );
        child.finish(None);
        parent.finish(None);
        let snapshot = timings.snapshot();
        assert!(entries(&snapshot, "active").is_empty());
        assert_eq!(entries(&snapshot, "records").len(), 2);
        assert!(
            entries(&snapshot, "records")
                .iter()
                .all(|record| record.get("state") == Some(&Value::string("ok")))
        );
        assert!(
            entries(&snapshot, "aggregates")
                .iter()
                .all(|aggregate| aggregate.get("exclusive_unknown_count")
                    == Some(&Value::Number(0.0)))
        );
        Ok(())
    }

    #[test]
    fn passive_timing_unions_overlapping_children_without_double_counting() {
        let second = Duration::from_secs;
        assert_eq!(
            exclusive(
                second(0),
                second(20),
                &[
                    (second(2), second(8)),
                    (second(6), second(12)),
                    (second(14), second(18))
                ]
            ),
            second(6)
        );
        assert_eq!(
            exclusive(second(5), second(10), &[(second(0), second(30))]),
            Duration::ZERO
        );
    }

    #[test]
    fn passive_timing_caps_completed_records_but_retains_aggregates() {
        let timings = Timings::default();
        for _ in 0..=MAX_RECORDS {
            timings.start(Label::SourceCapture).finish(None);
        }
        let snapshot = timings.snapshot();
        assert_eq!(entries(&snapshot, "records").len(), MAX_RECORDS);
        assert_eq!(snapshot.get("dropped_records"), Some(&Value::Number(1.0)));
        assert_eq!(
            entries(&snapshot, "aggregates")
                .first()
                .and_then(|aggregate| aggregate.get("count")),
            Some(&Value::Number(129.0))
        );
        assert!(!snapshot.json().contains("source body"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn passive_timing_pending_future_drop_closes_without_additional_polling()
    -> Result<(), Error> {
        let timings = Timings::default();
        let (send, ready) = tokio::sync::oneshot::channel();
        let observed = timings.clone();
        let mut future = Box::pin(async move {
            let _scope = observed.start(Label::Model);
            send.send(())
                .map_err(|_| Error::type_error("Timing readiness receiver dropped"))?;
            std::future::pending::<()>().await;
            Ok::<(), Error>(())
        });
        tokio::select! {
            biased;
            result = ready => result.map_err(|_| Error::type_error("Timing readiness not reached"))?,
            result = &mut future => { result?; return Err(Error::type_error("Pending timing future unexpectedly settled")); }
        }
        assert_eq!(entries(&timings.snapshot(), "active").len(), 1);
        drop(future);
        let snapshot = timings.snapshot();
        assert!(entries(&snapshot, "active").is_empty());
        assert_eq!(
            entries(&snapshot, "records")
                .first()
                .and_then(|record| record.get("state")),
            Some(&Value::string("scope-exit"))
        );
        Ok(())
    }
}
