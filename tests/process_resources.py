"""Optional process-tree sampling without command lines, environment or text.

RSS is summed and may double-count shared pages. PSS is reported only where all
live processes expose it. CPU is a sampled lower bound: very short descendants
and the final fraction of an interval can be missed. GPU/energy are unknown.
"""
import threading

try:
    import psutil as _psutil
except ImportError:
    _psutil = None


class ProcessTreeSampler:
    def __init__(self, pid, interval=.25):
        self.pid, self.interval = pid, interval
        self._stop = threading.Event()
        self._lock = threading.Lock()
        self._thread = None
        self._cpu = {}
        self._samples = 0
        self._rss = self._pss = None
        self._errors = 0
        self._root = None

    def start(self):
        if _psutil is not None:
            try:
                self._root = _psutil.Process(self.pid)
            except _psutil.Error:
                return self
            self._thread = threading.Thread(target=self._run, daemon=True)
            self._thread.start()
        return self

    def _sample(self):
        try:
            processes = [self._root, *self._root.children(recursive=True)]
        except _psutil.Error:
            return
        rss, pss, observed, all_pss = 0, 0, 0, True
        cpu = {}
        for process in processes:
            try:
                identity = (process.pid, process.create_time())
                times = process.cpu_times()
                cpu[identity] = times.user + times.system
                rss += process.memory_info().rss
                observed += 1
                try:
                    value = getattr(process.memory_full_info(), "pss", None)
                except (_psutil.Error, OSError):
                    value = None
                if value is None:
                    all_pss = False
                else:
                    pss += value
            except (_psutil.Error, OSError):
                all_pss = False
        if observed:
            with self._lock:
                self._samples += 1
                for identity, value in cpu.items():
                    self._cpu[identity] = max(value, self._cpu.get(identity, 0))
                self._rss = max(rss, self._rss or 0)
                if all_pss:
                    self._pss = max(pss, self._pss or 0)

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
            return dict(schema=1, method="psutil sampled descendant tree" if _psutil is not None else "unavailable: psutil not installed",
                        psutil_version=_psutil.__version__ if _psutil is not None else None,
                        interval_seconds=self.interval, samples=self._samples, processes_seen=len(self._cpu),
                        cpu_seconds_sampled=sum(self._cpu.values()) if self._samples else None,
                        peak_tree_rss_mib=self._rss / 1024**2 if self._rss is not None else None,
                        peak_tree_pss_mib=self._pss / 1024**2 if self._pss is not None else None,
                        gpu_memory_mib=None, energy_joules=None, sample_errors=self._errors,
                        sampler_stopped=self._thread is None or not self._thread.is_alive(),
                        limitation="Sampled CPU lower bound; short descendants/tails can be missed. Summed RSS can double-count shared pages; PSS/platform/GPU/energy may be unavailable. No command lines collected.")
