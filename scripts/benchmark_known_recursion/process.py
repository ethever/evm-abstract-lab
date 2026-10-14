"""Own one Linux process group and monitor resources outside blocking I/O."""

from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import select
import signal
import subprocess
import threading
import time


def utc():
    return datetime.now(timezone.utc).isoformat()


def sha256(path):
    digest = hashlib.sha256()
    with Path(path).open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def write_json(path, value):
    Path(path).write_text(json.dumps(value, indent=2) + "\n")


def available_memory():
    for line in Path("/proc/meminfo").read_text().splitlines():
        if line.startswith("MemAvailable:"):
            return int(line.split()[1])
    raise RuntimeError("Linux MemAvailable is unavailable")


class ProtectedProcess:
    """The monitor alone reaps the child; callers never use Popen.poll/wait."""

    def __init__(self, command, directory, root, guard, environment=None):
        self.directory, self.guard = Path(directory), guard
        self.started = time.monotonic()
        self.done = threading.Event()
        self.stop_lock = threading.Lock()
        self.stop_reason = None
        self.stop_at = None
        self.before_stop = None
        self.record = {"started_utc": utc(), "guard": guard, "exit_code": None}
        self.stdout = (self.directory / "stdout.log").open("wb")
        self.stderr = (self.directory / "stderr.log").open("wb")
        try:
            self.process = subprocess.Popen(
                command, cwd=root, env=environment, stdout=self.stdout,
                stderr=self.stderr, start_new_session=True,
            )
        except Exception:
            self.stdout.close()
            self.stderr.close()
            raise
        self.record["pid"] = self.process.pid
        try:
            self.pidfd = os.pidfd_open(self.process.pid)
        except (AttributeError, OSError):
            self.pidfd = None
        self.record["exit_observation"] = "pidfd wakes resource monitor" if self.pidfd is not None else "resource polling; wall includes exit observation delay"
        write_json(self.directory / "process-start.json", self.record)
        self.monitor = threading.Thread(target=self._monitor, daemon=True)
        self.monitor.start()

    def _sample(self):
        value = {"elapsed_seconds": time.monotonic() - self.started,
                 "host_MemAvailable_kib": available_memory()}
        try:
            for line in Path(f"/proc/{self.process.pid}/status").read_text().splitlines():
                if line.startswith(("VmRSS:", "VmHWM:", "VmSize:", "Threads:", "Cpus_allowed_list:")):
                    key, field = line.split(":", 1)
                    value[key] = field.strip()
            fields = Path(f"/proc/{self.process.pid}/stat").read_text().rsplit(")", 1)[1].split()
            ticks = os.sysconf("SC_CLK_TCK")
            value["user_seconds"] = int(fields[11]) / ticks
            value["system_seconds"] = int(fields[12]) / ticks
        except FileNotFoundError:
            value["process_disappeared"] = True
        return value

    def stop(self, reason):
        with self.stop_lock:
            if self.done.is_set() or self.process.returncode is not None or self.stop_reason:
                return
            self.stop_reason = reason
            self.stop_at = time.monotonic()
            self.record["stop_reason"] = reason
            if self.before_stop:
                try:
                    self.before_stop()
                except Exception as error:
                    self.record["cancel_before_stop_error_type"] = type(error).__name__
            try:
                os.killpg(self.process.pid, signal.SIGTERM)
                self.record["signal_sent"] = "SIGTERM to owned process group"
            except ProcessLookupError:
                pass

    def _monitor(self):
        next_message = 15
        try:
            with (self.directory / "resources.jsonl").open("w", buffering=1) as output:
                while True:
                    pid, status, usage = os.wait4(self.process.pid, os.WNOHANG)
                    if pid:
                        self.process.returncode = os.waitstatus_to_exitcode(status)
                        self.record.update(
                            exit_code=self.process.returncode,
                            wall_seconds=time.monotonic() - self.started,
                            user_seconds=usage.ru_utime, system_seconds=usage.ru_stime,
                            peak_rss_kib=usage.ru_maxrss, finished_utc=utc(),
                            termination=self.stop_reason or ("normal_exit" if self.process.returncode == 0 else "process_error"),
                        )
                        break
                    sample = self._sample()
                    output.write(json.dumps(sample) + "\n")
                    elapsed = sample["elapsed_seconds"]
                    if not self.stop_reason:
                        reason = None
                        if int(sample.get("VmRSS", "0").split()[0]) >= self.guard["rss_limit_kib"]:
                            reason = "external_rss_guard"
                        elif sample["host_MemAvailable_kib"] < self.guard["host_available_floor_kib"]:
                            reason = "external_host_memory_guard"
                        elif elapsed >= self.guard["timeout_seconds"]:
                            reason = "external_timeout"
                        if reason:
                            self.record["guard_stop_sample"] = sample
                            self.stop(reason)
                    elif time.monotonic() - self.stop_at >= 5:
                        try:
                            os.killpg(self.process.pid, signal.SIGKILL)
                            self.record["escalated_signal"] = "SIGKILL to owned process group after5s"
                        except ProcessLookupError:
                            pass
                    if elapsed >= next_message:
                        print(self.directory.name, f"elapsed={elapsed:.1f}s",
                              f"RSS={sample.get('VmRSS', 'unavailable')}", flush=True)
                        next_message = elapsed + 15
                    if self.pidfd is not None:
                        select.select([self.pidfd], [], [], self.guard["poll_seconds"])
                    else:
                        time.sleep(self.guard["poll_seconds"])
        except Exception as error:
            self.record["monitor_error_type"] = type(error).__name__
            try:
                os.killpg(self.process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            _, status, usage = os.wait4(self.process.pid, 0)
            self.process.returncode = os.waitstatus_to_exitcode(status)
            self.record.update(exit_code=self.process.returncode, termination="resource_monitor_error",
                               wall_seconds=time.monotonic() - self.started,
                               user_seconds=usage.ru_utime, system_seconds=usage.ru_stime,
                               peak_rss_kib=usage.ru_maxrss, finished_utc=utc())
        finally:
            if self.pidfd is not None:
                os.close(self.pidfd)
            self.stdout.close()
            self.stderr.close()
            self.record["owned_pid_absent"] = not Path(f"/proc/{self.process.pid}").exists()
            write_json(self.directory / "process.json", self.record)
            self.done.set()

    def finish(self, reason=None):
        if reason:
            self.stop(reason)
        self.done.wait()
        self.monitor.join()
        return self.record
