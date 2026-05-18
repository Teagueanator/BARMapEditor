# Runtime warnings — what's benign, what's actionable

This is the triage reference for warnings you may see at editor startup
on Linux. Pin it next to a bug report before opening an issue —
several of these are cosmetic noise from the wgpu / Mesa / kernel
stack and not signs of a real problem with the editor.

The default `tracing` filter (in `crates/barme-app/src/main.rs`)
already suppresses the two noisiest tracing-level events at the
`error`-only level. Some warnings below are emitted *outside* the
tracing system (printf from native libraries) — those will still
appear in your terminal.

## 1. `radv is not a conformant Vulkan implementation` (cosmetic)

**What you see:**
```
MESA: warning: radv is not a conformant Vulkan implementation, testing use only.
```

**Source:** Mesa's `radv` driver, printf to stderr. Not a tracing
event — we cannot suppress it without redirecting stderr.

**Meaning:** `radv` is the open-source AMD Vulkan driver. It is
fully functional but Khronos certification is held by AMD's
proprietary driver, so Mesa flags itself defensively. Every Vulkan
application on a radv-driven Linux machine sees this line. It is
the default driver shipped by Fedora, Ubuntu, etc.

**Action:** none. Pin it in your terminal scrollback as the first
filter when triaging real failures.

## 2. `VK_EXT_physical_device_drm` extension miss (benign)

**What you see (varies by Mesa version):**
```
... vkEnumerateInstanceExtensionProperties: missing VK_EXT_physical_device_drm
```

**Source:** wgpu's instance probe; emitted before the editor's logger
attaches stderr, so it doesn't go through the tracing filter.

**Meaning:** wgpu queries an optional extension that lets it inspect
which DRM render node each physical device backs. The probe is
non-fatal; wgpu falls back to vendor/device IDs.

**Action:** none. If you're chasing a multi-GPU bug, look at the
adapter info that `barme-app` logs immediately after init (`backend`,
`adapter`, `vendor`, `device_type`) — that has the data you need.

## 3. Validation layer not found

**What you see (with the default filter, this is now suppressed):**
```
wgpu_hal::vulkan: VALIDATION requested, but unable to find layer:
  VK_LAYER_KHRONOS_validation
```

**Source:** wgpu's Vulkan instance creation, when built in debug mode.
wgpu always asks for validation; the layer is only present if you
installed the system-wide Vulkan SDK.

**Meaning:** the editor's debug builds want Vulkan validation if
available, but it's an opt-in dev-time dependency, not a runtime
requirement. With the layer absent, the GPU still works fine — you
just don't get the extra validation diagnostics that catch driver-API
misuse.

**Action:**
- If you don't care about Vulkan validation: nothing. The default
  filter hides this line.
- If you want validation (e.g. you're debugging a wgpu issue):
  - Debian / Ubuntu: `sudo apt install vulkan-validationlayers`
  - Fedora: `sudo dnf install vulkan-validation-layers`
  - Arch: `sudo pacman -S vulkan-validation-layers`
  - Then unsuppress via `RUST_LOG='info,wgpu_hal::vulkan=warn'` or
    by editing `default_filter` in `crates/barme-app/src/main.rs`.

## 4. Wayland → GLES re-init

**What you see (with the default filter, this is now suppressed):**
```
wgpu_hal::gles::egl: Re-initializing Gles context due to Wayland window
```

**Source:** wgpu's GLES backend on Wayland. eframe's surface creation
sequence triggers a re-init the first time the window's actual
Wayland handle becomes available.

**Meaning:** harmless. The re-init completes synchronously before the
first frame renders.

**Action:** none. If you need to investigate a GLES issue, lift the
suppression with `RUST_LOG='info,wgpu_hal::gles::egl=warn'`.

## How to re-enable any suppressed warning

The default filter is a fallback used only when `RUST_LOG` is unset.
Setting `RUST_LOG` overrides it entirely. Examples:

```bash
# Everything wgpu emits at warn, plus our own info:
RUST_LOG='info,wgpu=warn,wgpu_hal=warn,wgpu_core=warn' cargo run -p barme-app

# Just the GLES re-init line:
RUST_LOG='warn,wgpu_hal::gles::egl=info' cargo run -p barme-app
```

`tracing-subscriber`'s `EnvFilter` syntax is documented at
<https://docs.rs/tracing-subscriber/latest/tracing_subscriber/filter/struct.EnvFilter.html>.
