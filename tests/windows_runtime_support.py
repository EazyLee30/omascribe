"""Windows component acceptance without CUDA SDK paths or driver overrides."""
import ctypes
from ctypes import wintypes
import os
from pathlib import Path
import re
import hashlib
import json

CUDA_LIBRARY = re.compile(r"(?:cudart|cublas(?:lt)?|nvrtc(?:-builtins)?)64_[0-9_]+\.dll", re.I)
OVERRIDES = {"VK_DRIVER_FILES", "VK_ICD_FILENAMES", "VK_ADD_DRIVER_FILES", "GGML_BACKEND_PATH"}


def component_identity(manifest, archive, server, backend):
    metadata = json.loads(Path(manifest).read_text(encoding="utf-8"))
    server = Path(server).resolve()
    def sha(path):
        with Path(path).open("rb") as stream:
            return hashlib.file_digest(stream, "sha256").hexdigest()
    if metadata["target"] != "windows-x86_64" or metadata["backend"] != backend or metadata["entrypoint"] != server.name:
        raise ValueError("Component platform/backend/entrypoint mismatch")
    if Path(archive).stat().st_size != metadata["archive"]["bytes"] or sha(archive) != metadata["archive"]["sha256"]:
        raise ValueError("Component archive checksum mismatch")
    expected = set()
    for member in metadata["files"]:
        name = member["name"]
        if Path(name).name != name or name in expected:
            raise ValueError("Invalid component member")
        expected.add(name)
        path = server.parent / name
        if path.stat().st_size != member["bytes"] or sha(path) != member["sha256"]:
            raise ValueError("Component member checksum mismatch: " + name)
    actual = {p.name for p in server.parent.iterdir()}
    if actual - {"installed.json"} != expected or server.name not in expected:
        raise ValueError("Runtime directory is not the exact component payload")
    build = json.loads((server.parent / "build-info.json").read_text(encoding="utf-8"))
    lock = json.loads((Path(__file__).resolve().parents[1] / "models.lock.json").read_text(encoding="utf-8"))
    if build["revision"] != lock["llama_cpp"]["revision"] or build["backend"] != backend or build["target"] != metadata["target"]:
        raise ValueError("Pinned runtime build identity mismatch")
    return dict(manifest_sha256=sha(manifest), archive_sha256=sha(archive), files_verified=len(expected),
                backend=backend, target=metadata["target"], runtime_revision=build["revision"], archive_name=metadata["archive"]["name"])


def acceptance_environment(runtime, original=None):
    env = dict(os.environ if original is None else original)
    removed = []
    for key in list(env):
        upper = key.upper()
        if (upper in OVERRIDES or upper.startswith(("CUDA", "CUDNN", "CUBLAS", "NVCC", "LLAMA_ARG_", "VK_", "GGML_"))
                or upper == "VULKAN_SDK"):
            removed.append(key)
            del env[key]
    system = Path(env.get("SystemRoot", r"C:\Windows"))
    env["PATH"] = os.pathsep.join(map(str, [Path(runtime).resolve(), system / "System32", system]))
    env.update(HF_HUB_OFFLINE="1", TRANSFORMERS_OFFLINE="1", OMP_NUM_THREADS="1", OPENBLAS_NUM_THREADS="1")
    return env, sorted(removed)


def cuda_driver_api():
    if os.name != "nt":
        raise RuntimeError("Native Windows acceptance only")
    driver = ctypes.WinDLL(str(Path(os.environ.get("SystemRoot", r"C:\Windows")) / "System32/nvcuda.dll"))
    driver.cuDriverGetVersion.argtypes = [ctypes.POINTER(ctypes.c_int)]
    driver.cuDriverGetVersion.restype = ctypes.c_int
    version = ctypes.c_int()
    result = driver.cuDriverGetVersion(ctypes.byref(version))
    if result:
        raise RuntimeError(f"cuDriverGetVersion failed: {result}")
    return version.value


def loaded_modules(pid):
    if os.name != "nt":
        raise RuntimeError("Native Windows module inspection only")
    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    psapi = ctypes.WinDLL("psapi", use_last_error=True)
    kernel.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
    kernel.OpenProcess.restype = wintypes.HANDLE
    kernel.CloseHandle.argtypes = [wintypes.HANDLE]
    psapi.EnumProcessModulesEx.argtypes = [wintypes.HANDLE, ctypes.POINTER(wintypes.HMODULE), wintypes.DWORD,
                                          ctypes.POINTER(wintypes.DWORD), wintypes.DWORD]
    psapi.GetModuleFileNameExW.argtypes = [wintypes.HANDLE, wintypes.HMODULE, wintypes.LPWSTR, wintypes.DWORD]
    handle = kernel.OpenProcess(0x0410, False, pid)  # Query information and read own child memory.
    if not handle:
        raise ctypes.WinError(ctypes.get_last_error())
    try:
        modules = (wintypes.HMODULE * 2048)()
        needed = wintypes.DWORD()
        if not psapi.EnumProcessModulesEx(handle, modules, ctypes.sizeof(modules), ctypes.byref(needed), 3):
            raise ctypes.WinError(ctypes.get_last_error())
        if needed.value > ctypes.sizeof(modules):
            raise RuntimeError("Module inventory exceeds the acceptance bound")
        result = []
        for module in modules[:needed.value // ctypes.sizeof(wintypes.HMODULE)]:
            path = ctypes.create_unicode_buffer(32768)
            if not psapi.GetModuleFileNameExW(handle, module, path, len(path)):
                raise ctypes.WinError(ctypes.get_last_error())
            result.append(Path(path.value))
        return result
    finally:
        kernel.CloseHandle(handle)


def audit_runtime_modules(pid, runtime, backend):
    runtime = Path(runtime).resolve()
    system = Path(os.environ.get("SystemRoot", r"C:\Windows")).resolve()
    observed = []
    for path in loaded_modules(pid):
        path = path.resolve()
        name = path.name.lower()
        if CUDA_LIBRARY.fullmatch(name):
            if path.parent != runtime:
                raise RuntimeError(f"CUDA dependency loaded outside component: {name}")
            observed.append(dict(name=name, source="component"))
        elif name in {"nvcuda.dll", "vulkan-1.dll"}:
            if not path.is_relative_to(system):
                raise RuntimeError(f"Driver loaded outside Windows driver installation: {name}")
            observed.append(dict(name=name, source="system_driver"))
    if backend == "cuda" and not any(m["name"] == "nvcuda.dll" for m in observed):
        raise RuntimeError("NVIDIA driver module was not observed")
    if backend == "vulkan" and not any(m["name"] == "vulkan-1.dll" for m in observed):
        raise RuntimeError("System Vulkan loader was not observed")
    return sorted(observed, key=lambda item: item["name"])
