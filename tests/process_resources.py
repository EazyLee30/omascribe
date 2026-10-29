"""Optional process-tree sampling without command lines, environment or text.

RSS is summed and may double-count shared pages. PSS is reported only where all
live processes expose it. CPU is a sampled lower bound: very short descendants
and the final fraction of an interval can be missed. GPU/energy are unknown.
Opt-in RSS integration uses trapezoids between adjacent recorded samples with
known RSS, without adding zero-valued endpoints or replacing unavailable values.
Processes disappearing during a sample can be missed, so tree sums are lower
bounds; permission/OS failures make that opt-in sample's memory values unknown.
"""
import threading
import time

try:
    import psutil as _psutil
except ImportError:
    _psutil = None

_MAX_TIMELINE_SAMPLES = 20000
_EXIT_CHECK_TIMEOUT_SECONDS = 2.0
_EXIT_CHECK_INTERVAL_SECONDS = .05


class ProcessTreeSampler:
    def __init__(self, pid, interval=.25, record_samples=False):
        self.pid, self.interval = pid, interval
        self.record_samples = record_samples
        self._stop = threading.Event()
        self._lock = threading.Lock()
        self._thread = None
        self._cpu = {}
        self._samples = 0
        self._rss = self._pss = None
        self._errors = 0
        self._root = None
        self._started_monotonic = None
        self._timeline = []
        self._observed = {}
        self._timeline_dropped = 0
        self._rss_integral = None

    def start(self):
        if self.record_samples:
            self._started_monotonic = time.monotonic()
        if _psutil is not None:
            try:
                self._root = _psutil.Process(self.pid)
            except _psutil.Error:
                return self
            self._thread = threading.Thread(target=self._run, daemon=True)
            self._thread.start()
        return self

    def _sample(self):
        elapsed, unix_ms = None, None
        if self.record_samples:
            now = time.monotonic()
            if self._started_monotonic is None:
                self._started_monotonic = now
            elapsed = max(0., now - self._started_monotonic)
            unix_ms = int(time.time() * 1000)
        try:
            processes = [self._root, *self._root.children(recursive=True)]
        except _psutil.Error as error:
            if self.record_samples:
                with self._lock:
                    if not isinstance(error, _psutil.NoSuchProcess):
                        self._errors += 1
                    self._record_sample(elapsed, unix_ms, None, None, 0)
            return
        rss, pss, observed, all_pss = 0, 0, 0, True
        cpu = {}
        identities = set()
        inaccessible, sample_errors = False, 0
        for process in processes:
            try:
                identity = (process.pid, process.create_time())
                identities.add(identity)
                times = process.cpu_times()
                cpu[identity] = times.user + times.system
                rss += process.memory_info().rss
                observed += 1
                try:
                    value = getattr(process.memory_full_info(), "pss", None)
                except (_psutil.Error, OSError) as error:
                    value = None
                    if self.record_samples and not isinstance(error, _psutil.NoSuchProcess):
                        inaccessible, sample_errors = True, sample_errors + 1
                if value is None:
                    all_pss = False
                else:
                    pss += value
            except (_psutil.Error, OSError) as error:
                all_pss = False
                if self.record_samples and not isinstance(error, _psutil.NoSuchProcess):
                    inaccessible, sample_errors = True, sample_errors + 1
        with self._lock:
            if observed:
                self._samples += 1
                for identity, value in cpu.items():
                    self._cpu[identity] = max(value, self._cpu.get(identity, 0))
                self._rss = max(rss, self._rss or 0)
                if all_pss:
                    self._pss = max(pss, self._pss or 0)
            if self.record_samples:
                self._errors += sample_errors
                for identity in identities:
                    self._observed[identity] = elapsed
                self._record_sample(elapsed, unix_ms,
                    rss / 1024**2 if observed and not inaccessible else None,
                    pss / 1024**2 if observed and all_pss and not inaccessible else None, len(identities))

    def _record_sample(self, elapsed, unix_ms, rss, pss, observed):
        """Called while holding the sampler lock; the retained series is bounded."""
        if len(self._timeline) >= _MAX_TIMELINE_SAMPLES:
            self._timeline_dropped += 1
            return
        sample = dict(elapsed_seconds=elapsed, unix_ms=unix_ms, tree_rss_mib=rss,
            tree_pss_mib=pss, cpu_seconds_sampled=sum(self._cpu.values()) if self._cpu else None,
            processes_observed=observed)
        if self._timeline:
            previous = self._timeline[-1]
            if previous["tree_rss_mib"] is not None and rss is not None:
                area = (previous["tree_rss_mib"] + rss) / 2 * (elapsed - previous["elapsed_seconds"])
                self._rss_integral = (self._rss_integral or 0.) + area
        self._timeline.append(sample)

    def _check_process_exits(self, identities):
        """Confirm sampled identities have exited, without signalling any process.

        An existing PID with a different creation time belongs to a new process.
        Permission errors remain unresolved, even if the polling deadline passes.
        """
        if _psutil is None or not identities:
            return False, [], 0
        pending = set(identities)
        alive, errors = set(), 0
        deadline = time.monotonic() + _EXIT_CHECK_TIMEOUT_SECONDS
        while pending:
            alive = set()
            for identity in tuple(pending):
                pid, created = identity
                try:
                    process = _psutil.Process(pid)
                    if process.create_time() != created or not process.is_running():
                        pending.remove(identity)
                    elif process.status() in (_psutil.STATUS_ZOMBIE, _psutil.STATUS_DEAD):
                        pending.remove(identity)
                    else:
                        alive.add(identity)
                except _psutil.NoSuchProcess:
                    pending.remove(identity)
                except (_psutil.Error, OSError):
                    errors += 1
            remaining = deadline - time.monotonic()
            if not pending or remaining <= 0:
                break
            time.sleep(min(_EXIT_CHECK_INTERVAL_SECONDS, remaining))
        return not pending, [dict(pid=pid, create_time=created) for pid, created in sorted(alive)], errors

    def _run(self):
        while not self._stop.is_set():
            try:
                self._sample()
            except Exception:
                with self._lock:
                    self._errors += 1
            self._stop.wait(self.interval)

    def finish(self):
        self._stop.set()
        if self._thread is not None:
            self._thread.join(timeout=1)
        with self._lock:
            result = dict(schema=1, method="psutil sampled descendant tree" if _psutil is not None else "unavailable: psutil not installed",
                        psutil_version=_psutil.__version__ if _psutil is not None else None,
                        interval_seconds=self.interval, samples=self._samples, processes_seen=len(self._cpu),
                        cpu_seconds_sampled=sum(self._cpu.values()) if self._samples else None,
                        peak_tree_rss_mib=self._rss / 1024**2 if self._rss is not None else None,
                        peak_tree_pss_mib=self._pss / 1024**2 if self._pss is not None else None,
                        gpu_memory_mib=None, energy_joules=None, sample_errors=self._errors,
                        sampler_stopped=self._thread is None or not self._thread.is_alive(),
                        limitation="Sampled CPU lower bound; short descendants/tails can be missed. Summed RSS can double-count shared pages; PSS/platform/GPU/energy may be unavailable. No command lines collected.")
            if self.record_samples:
                identities = dict(self._observed)
                result.update(timeline=list(self._timeline),
                    observed_processes=[dict(pid=pid, create_time=created, last_seen_elapsed_seconds=elapsed)
                        for (pid, created), elapsed in sorted(identities.items())],
                    timeline_truncated=bool(self._timeline_dropped), timeline_dropped_samples=self._timeline_dropped,
                    tree_rss_mib_seconds_sampled=self._rss_integral,
                    integration_method="Trapezoid between adjacent recorded samples with known RSS; no unsampled endpoints or unavailable values are imputed. Truncated timelines exclude dropped samples.")
        if self.record_samples:
            confirmed, alive, errors = self._check_process_exits(identities)
            result.update(processes_exit_confirmed=confirmed and result["sampler_stopped"],
                alive_after_finish=alive, exit_check_errors=errors)
        return result
