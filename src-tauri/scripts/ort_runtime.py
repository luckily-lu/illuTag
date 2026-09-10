import os
from pathlib import Path


def _nvidia_bin_dirs():
    dirs = []
    try:
        import nvidia

        root = Path(nvidia.__file__).resolve().parent
        for child in root.iterdir():
            if not child.is_dir():
                continue
            for name in ("bin", "lib", "lib/x64"):
                candidate = child / name
                if candidate.is_dir():
                    dirs.append(str(candidate))
    except Exception:
        pass
    return dirs


def prepare_ort():
    extra = _nvidia_bin_dirs()
    path_prefix = os.pathsep.join(extra)
    if path_prefix:
        os.environ["PATH"] = path_prefix + os.pathsep + os.environ.get("PATH", "")
    if hasattr(os, "add_dll_directory"):
        for directory in extra:
            try:
                os.add_dll_directory(directory)
            except OSError:
                pass
    import onnxruntime as ort

    try:
        ort.preload_dlls(cuda=True, cudnn=True, msvc=True, directory="")
    except TypeError:
        try:
            ort.preload_dlls()
        except Exception:
            pass
    except Exception:
        try:
            ort.preload_dlls()
        except Exception:
            pass
    return ort


def pick_providers(provider: str):
    ort = prepare_ort()
    available = ort.get_available_providers()
    force = os.environ.get("ILLUTAG_ORT_PROVIDER", "").strip().lower()
    if force == "cpu":
        return ["CPUExecutionProvider"]
    if "CUDAExecutionProvider" in available:
        return ["CUDAExecutionProvider", "CPUExecutionProvider"]
    if provider == "cuda":
        raise RuntimeError(
            "CUDAExecutionProvider not available. Install onnxruntime-gpu in the bundled Python."
        )
    return ["CPUExecutionProvider"]


def create_session(model_path, provider: str, log_prefix: str):
    import onnxruntime as ort

    providers = pick_providers(provider)
    try:
        session = ort.InferenceSession(model_path, providers=providers)
    except Exception as error:
        if providers and providers[0] == "CUDAExecutionProvider":
            print(
                f"[{log_prefix}] CUDA session failed, fallback CPU: {error}",
                file=__import__("sys").stderr,
                flush=True,
            )
            session = ort.InferenceSession(model_path, providers=["CPUExecutionProvider"])
        else:
            raise
    print(
        f"[{log_prefix}] providers={session.get_providers()}",
        file=__import__("sys").stderr,
        flush=True,
    )
    return session
