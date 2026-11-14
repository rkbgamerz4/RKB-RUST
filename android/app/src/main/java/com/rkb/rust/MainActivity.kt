package com.rkb.rust

import android.app.Activity
import android.graphics.Bitmap
import android.graphics.PixelFormat
import android.os.Build
import android.os.Bundle
import android.util.Log
import android.view.Gravity
import android.view.MotionEvent
import android.view.SurfaceHolder
import android.view.SurfaceView
import android.view.View
import android.view.WindowInsets
import android.view.WindowInsetsController
import android.view.WindowManager
import android.widget.TextView
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.util.concurrent.ConcurrentLinkedQueue

/**
 * Minimal host activity for the Rust engine.
 *
 * The Rust software renderer draws into a direct ByteBuffer; this activity
 * copies that buffer into an ARGB_8888 bitmap and presents it on a SurfaceView.
 */
class MainActivity : Activity() {
    private var view: RkbView? = null

    @Volatile
    private var activityResumed = false

    @Volatile
    private var engineHandle: Long = 0L

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        hideSystemUi()
        if (!RkbNative.isLoaded) {
            Log.e(TAG, "native library failed to load", RkbNative.loadError)
            showStartupError()
            return
        }
        Log.i(TAG, "RKB RUST native version ${RkbNative.nativeVersion()}")
        view = RkbView().also { setContentView(it) }
    }

    private fun showStartupError() {
        val message = TextView(this).apply {
            gravity = Gravity.CENTER
            text = getString(R.string.native_load_error)
            textSize = 18f
        }
        setContentView(message)
    }

    override fun onResume() {
        super.onResume()
        hideSystemUi()
        activityResumed = true
        val current = view ?: return
        if (engineHandle != 0L) {
            RkbNative.nativeResume(engineHandle)
        }
        current.startRendering()
    }

    override fun onPause() {
        activityResumed = false
        val current = view
        val stopped = current?.stopRendering() ?: true
        if (stopped && engineHandle != 0L) {
            RkbNative.nativePause(engineHandle)
        }
        super.onPause()
    }

    override fun onDestroy() {
        val stopped = view?.stopRendering() ?: true
        if (stopped) {
            if (engineHandle != 0L) {
                RkbNative.nativeDestroy(engineHandle)
                engineHandle = 0L
            }
        } else {
            // Leaking the handle is safer than freeing it while the render
            // thread might still be inside a native call.
            Log.w(TAG, "render thread did not stop; leaving native engine allocated")
        }
        view = null
        super.onDestroy()
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        if (hasFocus) {
            hideSystemUi()
        }
    }

    private fun hideSystemUi() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            window.insetsController?.let { controller ->
                controller.hide(WindowInsets.Type.statusBars() or WindowInsets.Type.navigationBars())
                controller.systemBarsBehavior =
                    WindowInsetsController.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
            }
        } else {
            @Suppress("DEPRECATION")
            window.decorView.systemUiVisibility =
                View.SYSTEM_UI_FLAG_IMMERSIVE_STICKY or
                View.SYSTEM_UI_FLAG_FULLSCREEN or
                View.SYSTEM_UI_FLAG_HIDE_NAVIGATION or
                View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN or
                View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION or
                View.SYSTEM_UI_FLAG_LAYOUT_STABLE
        }
    }

    private inner class RkbView : SurfaceView(this), SurfaceHolder.Callback, Runnable {
        @Volatile
        private var running = false

        @Volatile
        private var surfaceReady = false

        private var renderThread: Thread? = null
        private var frameBuffer: ByteBuffer? = null
        private var bitmap: Bitmap? = null

        @Volatile
        private var bufferWidth = 0

        @Volatile
        private var bufferHeight = 0

        @Volatile
        private var lastFrameNanos = 0L

        private var frameCount = 0L
        private val touchQueue = ConcurrentLinkedQueue<TouchSample>()

        init {
            holder.addCallback(this)
            holder.setFormat(PixelFormat.RGBA_8888)
            keepScreenOn = true
            isFocusable = true
            isFocusableInTouchMode = true
        }

        override fun surfaceCreated(holder: SurfaceHolder) {
            surfaceReady = true
            if (activityResumed) {
                startRendering()
            }
        }

        override fun surfaceChanged(holder: SurfaceHolder, format: Int, width: Int, height: Int) {
            // Force the render thread to rebuild its bitmap and direct buffer
            // for the new surface geometry.
            synchronized(this) {
                bufferWidth = 0
                bufferHeight = 0
            }
        }

        override fun surfaceDestroyed(holder: SurfaceHolder) {
            surfaceReady = false
            touchQueue.clear()
            lastFrameNanos = 0L
            if (stopRendering()) {
                releaseBuffers()
            }
        }

        fun startRendering() {
            if (renderThread?.isAlive == true) {
                return
            }
            // Do not carry a frame delta across a pause or surface recreation.
            lastFrameNanos = 0L
            running = true
            renderThread = Thread(this, "RkbRender").also { it.start() }
        }

        fun stopRendering(): Boolean {
            running = false
            touchQueue.clear()
            val thread = renderThread
            if (thread != null && thread !== Thread.currentThread()) {
                try {
                    thread.join(1000)
                } catch (interrupted: InterruptedException) {
                    Thread.currentThread().interrupt()
                }
            }
            val stopped = thread == null || !thread.isAlive
            if (stopped) {
                renderThread = null
            } else {
                Log.w(TAG, "render thread did not stop within 1s")
            }
            return stopped
        }

        private fun releaseBuffers() {
            frameBuffer = null
            bitmap?.recycle()
            bitmap = null
            bufferWidth = 0
            bufferHeight = 0
        }

        override fun run() {
            while (running && surfaceReady) {
                val frameStart = System.nanoTime()
                val canvas = holder.lockCanvas()
                if (canvas == null) {
                    sleepUntilNextFrame(frameStart)
                    continue
                }
                try {
                    val width = canvas.width
                    val height = canvas.height
                    if (width <= 0 || height <= 0) {
                        sleepUntilNextFrame(frameStart)
                        continue
                    }
                    ensureBuffers(width, height)

                    if (engineHandle == 0L) {
                        engineHandle = RkbNative.nativeCreate(width, height)
                        if (engineHandle == 0L) {
                            Log.e(TAG, "nativeCreate failed")
                            running = false
                            break
                        }
                    }

                    // Native calls are confined to this render thread. UI-thread
                    // touch events are queued and delivered here.
                    drainTouchQueue(engineHandle)

                    val now = System.nanoTime()
                    val delta = if (lastFrameNanos == 0L) {
                        FRAME_NANOS
                    } else {
                        (now - lastFrameNanos).coerceIn(1_000_000L, 250_000_000L)
                    }
                    lastFrameNanos = now

                    val buffer = frameBuffer
                    val target = bitmap
                    if (buffer != null && target != null) {
                        if (RkbNative.nativeRenderFrame(engineHandle, buffer, width, height, delta)) {
                            buffer.rewind()
                            target.copyPixelsFromBuffer(buffer)
                            canvas.drawBitmap(target, 0f, 0f, null)
                            frameCount++
                            if (frameCount % 300L == 0L) {
                                Log.i(TAG, RkbNative.nativeStats(engineHandle))
                            }
                        }
                    }
                } catch (error: Throwable) {
                    Log.e(TAG, "render loop failed", error)
                    running = false
                } finally {
                    try {
                        holder.unlockCanvasAndPost(canvas)
                    } catch (error: IllegalStateException) {
                        Log.w(TAG, "surface released before present", error)
                    }
                }

                sleepUntilNextFrame(frameStart)
            }
        }

        private fun sleepUntilNextFrame(frameStart: Long) {
            val remaining = FRAME_NANOS - (System.nanoTime() - frameStart)
            if (remaining > 0L) {
                try {
                    Thread.sleep(remaining / 1_000_000L, (remaining % 1_000_000L).toInt())
                } catch (interrupted: InterruptedException) {
                    Thread.currentThread().interrupt()
                    running = false
                }
            }
        }

        private fun ensureBuffers(width: Int, height: Int) {
            if (width == bufferWidth && height == bufferHeight && frameBuffer != null) {
                return
            }
            val bytes = width.toLong() * height.toLong() * 4L
            check(bytes in 4..Int.MAX_VALUE.toLong()) {
                "invalid surface size ${width}x$height"
            }
            frameBuffer = ByteBuffer.allocateDirect(bytes.toInt()).order(ByteOrder.nativeOrder())
            val oldBitmap = bitmap
            bitmap = Bitmap.createBitmap(width, height, Bitmap.Config.ARGB_8888)
            oldBitmap?.recycle()
            bufferWidth = width
            bufferHeight = height
        }

        override fun onTouchEvent(event: MotionEvent): Boolean {
            when (event.actionMasked) {
                MotionEvent.ACTION_MOVE,
                MotionEvent.ACTION_CANCEL,
                -> {
                    for (index in 0 until event.pointerCount) {
                        enqueueTouch(
                            event.actionMasked,
                            event.getPointerId(index),
                            event.getX(index),
                            event.getY(index),
                            event.eventTime,
                        )
                    }
                }
                MotionEvent.ACTION_DOWN,
                MotionEvent.ACTION_POINTER_DOWN,
                MotionEvent.ACTION_UP,
                MotionEvent.ACTION_POINTER_UP,
                -> {
                    val index = event.actionIndex
                    enqueueTouch(
                        event.actionMasked,
                        event.getPointerId(index),
                        event.getX(index),
                        event.getY(index),
                        event.eventTime,
                    )
                }
                else -> Unit
            }
            return true
        }

        private fun enqueueTouch(
            action: Int,
            pointerId: Int,
            x: Float,
            y: Float,
            timeMs: Long,
        ) {
            // Bound the queue if no surface is consuming events yet.
            while (touchQueue.size >= MAX_QUEUED_TOUCHES) {
                touchQueue.poll()
            }
            touchQueue.offer(TouchSample(action, pointerId.toLong(), x, y, timeMs))
        }

        private fun drainTouchQueue(handle: Long) {
            if (handle == 0L) {
                return
            }
            while (true) {
                val sample = touchQueue.poll() ?: return
                RkbNative.nativeTouch(
                    handle,
                    sample.action,
                    sample.pointerId,
                    sample.x,
                    sample.y,
                    sample.timeMs,
                )
            }
        }

        override fun performClick(): Boolean {
            super.performClick()
            return true
        }
    }

    private data class TouchSample(
        val action: Int,
        val pointerId: Long,
        val x: Float,
        val y: Float,
        val timeMs: Long,
    )

    private companion object {
        const val TAG = "RKB-RUST"
        const val FRAME_NANOS = 16_666_667L
        const val MAX_QUEUED_TOUCHES = 512
    }
}
