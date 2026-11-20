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
    external fun nativeJoinServer(
        handle: Long,
        server: String,
        name: String,
        registryPath: String,
        resourcePack: String,
    ): Boolean
    external fun nativePause(handle: Long): Boolean
    external fun nativeResume(handle: Long): Boolean
    external fun nativeStats(handle: Long): String
    external fun nativeVersion(): String

    /**
     * Attaches the GPU swapchain to [surface] and initializes wgpu.
     *
     * Returns the adapter summary (`wgpu / Vulkan / <name> hardware GPU` or
     * `… software renderer`) on success and `null` on failure, in which case
     * [nativeGpuStatus] reports the exact error and the caller must fall back
     * to the M6 software bitmap path.
     */
    external fun nativeAttachSurface(
        handle: Long,
        surface: android.view.Surface,
        width: Int,
        height: Int,
    ): String?

    /** Drops the swapchain and releases the native window reference. */
    external fun nativeDetachSurface(handle: Long): Boolean

    /**
     * Renders one frame into the attached swapchain.
     *
     * 0 = presented, 1 = skipped (paused or no update), 2 = failed; on failure
     * the caller falls back to the software renderer.
     */
    external fun nativeRenderFrameGpu(
        handle: Long,
        width: Int,
        height: Int,
        deltaNanos: Long,
    ): Int

    /** Active backend line, including the exact GPU error when one occurred. */
    external fun nativeGpuStatus(handle: Long): String
}
