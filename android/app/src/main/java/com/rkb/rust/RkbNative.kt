package com.rkb.rust

/**
 * JNI surface implemented by `crates/rkb-android`.
 *
 * Native state is not synchronized. The activity serializes calls: touch input
 * is queued for the render thread, and lifecycle calls happen only after the
 * render thread has been stopped and joined.
 *
 * Library loading is captured instead of thrown from the object initializer so
 * a missing or ABI-mismatched library produces a readable screen rather than a
 * process crash. Callers must check [isLoaded] before invoking externals.
 */
object RkbNative {
    @Volatile
    var loadError: Throwable? = null
        private set

    @Volatile
    private var loaded = false

    val isLoaded: Boolean
        get() = loaded

    init {
        try {
            System.loadLibrary("rkb_android")
            loaded = true
        } catch (error: Throwable) {
            loadError = error
        }
    }

    external fun nativeCreate(width: Int, height: Int, worldPath: String): Long
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
