//! Android integration for RKB RUST.
//!
//! Exposes a small JNI surface consumed by `RkbNative.kt`. The Rust side owns
//! the runtime and renders into a Kotlin-provided direct `ByteBuffer`.
//!
//! # Threading
//!
//! Every native function must be called from the same UI/render thread. The
//! Kotlin wrapper guarantees this with a single render thread. Handles are
//! heap-allocated [`AndroidEngine`] pointers and must be destroyed exactly once.

#![allow(clippy::missing_safety_doc)]

mod bridge;

pub use bridge::{AndroidEngine, DirectBufferSurface};

use std::time::Duration;

use ::jni::objects::{JByteBuffer, JClass};
use ::jni::sys::{jboolean, jint, jlong, jstring, JNI_FALSE, JNI_TRUE};
use ::jni::JNIEnv;

use bridge::{frame_bytes, touch_phase, EngineFrame};

const LOG_ERROR: i32 = 6;
const LOG_INFO: i32 = 4;

fn log(priority: i32, message: &str) {
    #[cfg(target_os = "android")]
    {
        use std::ffi::c_char;

        unsafe extern "C" {
            fn __android_log_write(prio: i32, tag: *const c_char, text: *const c_char) -> i32;
        }

        let mut text = message.as_bytes().to_vec();
        text.push(0);
        // SAFETY: `text` and the static tag are NUL-terminated and live for the call.
        unsafe {
            __android_log_write(priority, c"RKB-RUST".as_ptr(), text.as_ptr().cast());
        }
    }

    #[cfg(not(target_os = "android"))]
    {
        let level = if priority >= LOG_ERROR {
            "error"
        } else {
            "info"
        };
        eprintln!("[RKB-RUST/{level}] {message}");
    }
}

/// # Safety
///
/// `handle` must be zero or a pointer previously returned by `nativeCreate`
/// that has not been destroyed.
unsafe fn engine_mut<'a>(handle: jlong) -> Option<&'a mut AndroidEngine> {
    if handle == 0 {
        return None;
    }
    let pointer = handle as *mut AndroidEngine;
    if pointer.is_null() {
        return None;
    }
    // SAFETY: upheld by the caller contract; JNI is single-threaded here.
    Some(unsafe { &mut *pointer })
}

#[no_mangle]
pub extern "system" fn Java_com_rkb_rust_RkbNative_nativeCreate(
    _env: JNIEnv,
    _class: JClass,
    width: jint,
    height: jint,
) -> jlong {
    let width = u32::try_from(width.max(1)).unwrap_or(1);
    let height = u32::try_from(height.max(1)).unwrap_or(1);
    match AndroidEngine::new(width, height) {
        Ok(engine) => {
            log(
                LOG_INFO,
                &format!("native engine created ({width}x{height})"),
            );
            Box::into_raw(Box::new(engine)) as jlong
        }
        Err(error) => {
            log(LOG_ERROR, &format!("native engine failed: {error}"));
            0
        }
    }
}

#[no_mangle]
pub extern "system" fn Java_com_rkb_rust_RkbNative_nativeDestroy(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) {
    if handle == 0 {
        return;
    }
    // SAFETY: handle originates from nativeCreate and Kotlin destroys once.
    let mut engine = unsafe { Box::from_raw(handle as *mut AndroidEngine) };
    engine.destroy();
    log(LOG_INFO, "native engine destroyed");
}

#[no_mangle]
pub extern "system" fn Java_com_rkb_rust_RkbNative_nativeResize(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
    width: jint,
    height: jint,
) -> jboolean {
    let Some(engine) = (unsafe { engine_mut(handle) }) else {
        return JNI_FALSE;
    };
    engine.resize(
        u32::try_from(width.max(1)).unwrap_or(1),
        u32::try_from(height.max(1)).unwrap_or(1),
    );
    JNI_TRUE
}

#[no_mangle]
pub extern "system" fn Java_com_rkb_rust_RkbNative_nativeRenderFrame(
    env: JNIEnv,
    _class: JClass,
    handle: jlong,
    buffer: JByteBuffer,
    width: jint,
    height: jint,
    delta_nanos: jlong,
) -> jboolean {
    let Some(engine) = (unsafe { engine_mut(handle) }) else {
        return JNI_FALSE;
    };
    let width = u32::try_from(width.max(1)).unwrap_or(1);
    let height = u32::try_from(height.max(1)).unwrap_or(1);
    let Some(expected) = frame_bytes(width, height) else {
        log(LOG_ERROR, "frame dimensions overflow");
        return JNI_FALSE;
    };

    let address = match env.get_direct_buffer_address(&buffer) {
        Ok(address) if !address.is_null() => address,
        _ => {
            log(LOG_ERROR, "frame buffer is not a direct ByteBuffer");
            return JNI_FALSE;
        }
    };
    let capacity = env.get_direct_buffer_capacity(&buffer).unwrap_or(0);
    if capacity < expected {
        log(
            LOG_ERROR,
            &format!("frame buffer too small: {capacity} < {expected}"),
        );
        return JNI_FALSE;
    }

    // SAFETY: JNI guarantees a direct buffer of `capacity` writable bytes for
    // the duration of this call, and capacity >= expected.
    let pixels = unsafe { std::slice::from_raw_parts_mut(address, expected) };
    let delta = Duration::from_nanos(u64::try_from(delta_nanos.max(0)).unwrap_or(0));
    match engine.frame(pixels, width, height, delta) {
        EngineFrame::Presented(_) => JNI_TRUE,
        EngineFrame::Skipped => JNI_TRUE,
        EngineFrame::Failed(error) => {
            log(LOG_ERROR, &error);
            JNI_FALSE
        }
    }
}

#[no_mangle]
pub extern "system" fn Java_com_rkb_rust_RkbNative_nativeTouch(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
    action: jint,
    pointer_id: jlong,
    x: f32,
    y: f32,
    time_ms: jlong,
) -> jboolean {
    let Some(engine) = (unsafe { engine_mut(handle) }) else {
        return JNI_FALSE;
    };
    let timestamp = Duration::from_millis(u64::try_from(time_ms.max(0)).unwrap_or(0));
    match touch_phase(action) {
        Some(phase) => {
            engine.touch(phase, pointer_id as u64, x, y, timestamp);
            JNI_TRUE
        }
        None => JNI_FALSE,
    }
}

#[no_mangle]
pub extern "system" fn Java_com_rkb_rust_RkbNative_nativePause(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jboolean {
    let Some(engine) = (unsafe { engine_mut(handle) }) else {
        return JNI_FALSE;
    };
    if engine.pause().is_ok() {
        JNI_TRUE
    } else {
        JNI_FALSE
    }
}

#[no_mangle]
pub extern "system" fn Java_com_rkb_rust_RkbNative_nativeResume(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jboolean {
    let Some(engine) = (unsafe { engine_mut(handle) }) else {
        return JNI_FALSE;
    };
    if engine.resume().is_ok() {
        JNI_TRUE
    } else {
        JNI_FALSE
    }
}

#[no_mangle]
pub extern "system" fn Java_com_rkb_rust_RkbNative_nativeStats(
    env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jstring {
    let summary = (unsafe { engine_mut(handle) })
        .map(|engine| engine.stats_text())
        .unwrap_or_else(|| "no-engine".to_owned());
    env.new_string(summary)
        .map_or(std::ptr::null_mut(), ::jni::objects::JString::into_raw)
}

#[no_mangle]
pub extern "system" fn Java_com_rkb_rust_RkbNative_nativeVersion(
    env: JNIEnv,
    _class: JClass,
) -> jstring {
    env.new_string(rkb_core::VERSION)
        .map_or(std::ptr::null_mut(), ::jni::objects::JString::into_raw)
}
