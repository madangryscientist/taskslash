"""A job of known cost, for measuring the measurement.

    python3 probe.py <cpu_seconds_per_process> <mem_mib> <write_mib> <dir>

Starts two processes that each burn <cpu_seconds_per_process> of their own CPU time (CPU time, not
wall time, so a loaded machine changes how long it takes, not how much CPU it uses). While they
run, holds <mem_mib> MiB with every page written, and writes and fsyncs a <write_mib> MiB file in
<dir>, removed at the end.
"""

import multiprocessing
import os
import sys
import time


def burn(cpu_seconds):
    while time.process_time() < cpu_seconds:
        pass


def main():
    cpu_seconds = float(sys.argv[1])
    mem_mib = int(sys.argv[2])
    write_mib = int(sys.argv[3])
    directory = sys.argv[4]

    workers = [multiprocessing.Process(target=burn, args=(cpu_seconds,)) for _ in range(2)]
    for worker in workers:
        worker.start()

    # The multiply writes every page; bytearray(n) would leave them untouched.
    held = bytearray(b"\x01") * (mem_mib << 20)

    # Each MiB is fsynced and dropped from the page cache, which the job's memory is charged for,
    # so the peak memory stays the held buffer plus the interpreters.
    path = os.path.join(directory, "probe-%d.bin" % os.getpid())
    chunk = b"\x02" * (1 << 20)
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    try:
        for _ in range(write_mib):
            os.write(fd, chunk)
            os.fsync(fd)
            os.posix_fadvise(fd, 0, 0, os.POSIX_FADV_DONTNEED)
        os.close(fd)
        for worker in workers:
            worker.join()
    finally:
        os.remove(path)
    del held
    return max(worker.exitcode or 0 for worker in workers)


if __name__ == "__main__":
    sys.exit(main())
