use std::env;
use std::path::PathBuf;

fn main() {
    let dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());

    // Platform defines matching the SDK's ConfigureAbletonLink.cmake
    let mut build = cc::Build::new();
    build
        .cpp(true)
        .file(dir.join("bridge.cpp"))
        .include(dir.join("include"))
        .include(dir.join("asio-include"))
        .std("c++17");

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap();
    let target_env = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    match target_os.as_str() {
        "macos" => {
            build.define("LINK_PLATFORM_MACOSX", "1");
        }
        "windows" => {
            build.define("LINK_PLATFORM_WINDOWS", "1");
            build.define("INCL_EXTRA_HTON_FUNCTIONS", "1");
            build.define("_WIN32_WINNT", "0x0601");

            // The SDK includes Windows headers in camel case; MinGW headers
            // are lowercase and Linux/macOS filesystems are case-sensitive.
            // Provide shims so the Windows GNU cross-build works like MSVC.
            if target_env != "msvc" {
                let shim = PathBuf::from(env::var("OUT_DIR").unwrap()).join("win-compat");
                std::fs::create_dir_all(&shim).unwrap();
                for (sdk_name, real_name) in [
                    ("WS2tcpip.h", "ws2tcpip.h"),
                    ("WinSock2.h", "winsock2.h"),
                    ("Windows.h", "windows.h"),
                ] {
                    std::fs::write(
                        shim.join(sdk_name),
                        format!("#include <{real_name}>\n"),
                    )
                    .unwrap();
                }
                build.include(&shim);
                // MinGW's SDK headers do not declare SetThreadDescription
                // (Windows 10 only). Thread naming is cosmetic for Link.
                build.flag("-DSetThreadDescription(a,b)=((void)0)");
            }
        }
        "linux" | "android" => {
            build.define("LINK_PLATFORM_LINUX", "1");
        }
        _ => {}
    }

    // Link needs C++ exceptions + threads
    if target_os == "macos" {
        build.flag("-std=c++17").flag("-stdlib=libc++");
    }

    build.compile("ableton_link_bridge");

    // Propagate linker flags: C++ standard library + platform deps
    if target_os == "macos" {
        println!("cargo:rustc-link-lib=c++");
    } else {
        println!("cargo:rustc-link-lib=stdc++");
    }
    if target_os == "linux" {
        println!("cargo:rustc-link-lib=pthread");
    }
    if target_os == "windows" {
        println!("cargo:rustc-link-lib=ws2_32");
        println!("cargo:rustc-link-lib=iphlpapi");
    }

    println!("cargo:rerun-if-changed=bridge.cpp");
    println!("cargo:rerun-if-changed=include/ableton/Link.hpp");
}
