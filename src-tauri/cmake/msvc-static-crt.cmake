# Windows (MSVC) toolchain file for the CMake-built C++ deps (llama.cpp): use
# the static C/C++ runtime (/MT), matching the rest of the app
# (.cargo/config.toml). llama.cpp's CMake policies pick the runtime from
# CMAKE_MSVC_RUNTIME_LIBRARY and ignore the /MT that cmake-rs passes in flags.
set(CMAKE_MSVC_RUNTIME_LIBRARY "MultiThreaded")
