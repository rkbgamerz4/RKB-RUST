package com.rkb.rust

/**
 * JNI surface implemented by `crates/rkb-android`.
 *
 * All methods must be called from the render thread, except nativeVersion.
 */
object RkbNative {
    init {
        System.loadLibrary("rkb_android")
    }

    external fun nativeCreate(width: Int, height: Int): Long
    external fun nativeDestroy(handle: Long)
    external fun nativeResize(handle: Long, width: Int, height: Int): Boolean
    external fun nativeRenderFrame(
        handle: Long,
        buffer: java.nio.ByteBuffer,
        width: Int,
        height: Int,
        deltaNanos: Long,
    ): Boolean
    external fun nativeTouch(
        handle: Long,
        action: Int,
        pointerId: Long,
        x: Float,
        y: Float,
        timeMs: Long,
    ): Boolean
    external fun nativePause(handle: Long): Boolean
    external fun nativeResume(handle: Long): Boolean
    external fun nativeStats(handle: Long): String
    external fun nativeVersion(): String
}
