"""Windows native counters. Imported only on Windows, only for diagnostics.

PrivateUsage is commit, not private working set. EX2 supplies the latter on
Windows 10/11 22H2 with the September 2023 update. Older systems return null
for that counter rather than substituting total working set or private bytes.
See docs/memory-measurement.md for the Microsoft API references.
"""
import ctypes as c
from ctypes import wintypes as w


class Entry(c.Structure):
    _fields_ = [("dwSize", w.DWORD), ("cntUsage", w.DWORD), ("pid", w.DWORD),
                ("heap", c.c_size_t), ("module", w.DWORD), ("threads", w.DWORD),
                ("ppid", w.DWORD), ("priority", w.LONG), ("flags", w.DWORD),
                ("name", w.WCHAR * 260)]


class Counters(c.Structure):
    _fields_ = [("cb", w.DWORD), ("PageFaultCount", w.DWORD)] + [
        (name, c.c_size_t) for name in ("PeakWorkingSetSize", "WorkingSetSize",
        "QuotaPeakPagedPoolUsage", "QuotaPagedPoolUsage", "QuotaPeakNonPagedPoolUsage",
        "QuotaNonPagedPoolUsage", "PagefileUsage", "PeakPagefileUsage", "PrivateUsage")]


class Counters2(c.Structure):
    _fields_ = Counters._fields_ + [("PrivateWorkingSetSize", c.c_size_t),
                                   ("SharedCommitUsage", c.c_ulonglong)]


def kernel():
    dll = c.WinDLL("kernel32", use_last_error=True)
    for name, args, result in (
        ("CreateToolhelp32Snapshot", [w.DWORD, w.DWORD], w.HANDLE),
        ("Process32FirstW", [w.HANDLE, c.POINTER(Entry)], w.BOOL),
        ("Process32NextW", [w.HANDLE, c.POINTER(Entry)], w.BOOL),
        ("OpenProcess", [w.DWORD, w.BOOL, w.DWORD], w.HANDLE),
        ("CloseHandle", [w.HANDLE], w.BOOL),
        ("K32GetProcessMemoryInfo", [w.HANDLE, c.c_void_p, w.DWORD], w.BOOL),
    ):
        fn = getattr(dll, name)
        fn.argtypes, fn.restype = args, result
    return dll


def processes():
    dll = kernel()
    handle = dll.CreateToolhelp32Snapshot(2, 0)  # TH32CS_SNAPPROCESS
    if handle == c.c_void_p(-1).value:
        raise c.WinError(c.get_last_error())
    try:
        entry = Entry()
        entry.dwSize = c.sizeof(entry)
        found = dll.Process32FirstW(handle, c.byref(entry))
        rows = {}
        while found:
            rows[entry.pid] = {"ppid": entry.ppid, "name": entry.name}
            found = dll.Process32NextW(handle, c.byref(entry))
        if c.get_last_error() != 18:  # ERROR_NO_MORE_FILES
            raise c.WinError(c.get_last_error())
        return rows
    finally:
        dll.CloseHandle(handle)


def memory(pid):
    dll = kernel()
    handle = dll.OpenProcess(0x1000, False, pid)  # PROCESS_QUERY_LIMITED_INFORMATION
    if not handle:
        raise c.WinError(c.get_last_error())
    try:
        # A sentinel also catches systems that accept EX2 but fill only EX.
        counters = Counters2()
        counters.cb = c.sizeof(counters)
        counters.PrivateWorkingSetSize = c.c_size_t(-1).value
        if dll.K32GetProcessMemoryInfo(handle, c.byref(counters), counters.cb):
            private = counters.PrivateWorkingSetSize
            unavailable = (private == c.c_size_t(-1).value or private > counters.WorkingSetSize
                           or private == 0 < counters.WorkingSetSize)
            return {"commit_bytes": counters.PrivateUsage,
                    "private_working_set_bytes": None if unavailable else private}
        counters = Counters()
        counters.cb = c.sizeof(counters)
        if not dll.K32GetProcessMemoryInfo(handle, c.byref(counters), counters.cb):
            raise c.WinError(c.get_last_error())
        return {"commit_bytes": counters.PrivateUsage, "private_working_set_bytes": None}
    finally:
        dll.CloseHandle(handle)
