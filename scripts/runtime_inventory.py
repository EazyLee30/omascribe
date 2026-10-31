"""Base runtime gate: optional GPU dependencies are delivered separately."""
import pathlib
import re

OPTIONAL_GPU = re.compile(r"(?:^|[/\\])(?:accelerators?(?:[/\\]|$)|(?:lib)?(?:ggml[-_](?:cuda|vulkan|hip|sycl)|cuda(?:rt)?|cublas(?:lt)?|cudnn|cufft|curand|cusolver|cusparse|nvrtc|nvcc|nvcuda|nvjitlink|nvfatbin|nvml|nvidia|amdhip|hipblas|rocblas|rocm|miopen|amd_comgr|sycl|vulkan|DirectML|onnxruntime_providers_(?:cuda|rocm|dml|tensorrt))[^/\\]*\.(?:dll|so(?:\.[0-9]+)*|dylib|exe))", re.I)


def assert_base_names(names):
    for name in names:
        if OPTIONAL_GPU.search(str(name)):
            raise ValueError(f"Optional acceleration dependency in base payload: {name}")


def assert_base_runtime(directory):
    directory = pathlib.Path(directory)
    assert_base_names(path.relative_to(directory) for path in directory.rglob("*"))
