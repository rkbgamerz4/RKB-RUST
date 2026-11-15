package com.rkb.rust

import android.app.Activity
import android.graphics.Bitmap
import android.graphics.Color
import android.graphics.PixelFormat
import android.graphics.Rect
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
import android.widget.FrameLayout
import android.widget.TextView
import java.io.File
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
    private var overlay: TextView? = null

    @Volatile
    private var activityResumed = false

    @Volatile
    private var engineHandle: Long = 0L

    /** User save location. ADB can push a 1.21 world here on debuggable builds. */
    private val worldDir: File by lazy {
        val external = getExternalFilesDir(null)?.let { File(it, "minecraft/world") }
        val internal = File(filesDir, "minecraft/world")
        val hasSave = { dir: File? -> dir?.let { File(it, "level.dat").isFile } == true }
        when {
            hasSave(external) -> external!!
            hasSave(internal) -> internal
            else -> external ?: internal
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        try {
            if (!RkbNative.isLoaded) {
                Log.e(TAG, "native library failed to load", RkbNative.loadError)
                showStartupError(getString(R.string.native_load_error))
                return
            }
            try {
                worldDir.parentFile?.mkdirs()
            } catch (error: SecurityException) {
                Log.w(TAG, "could not create the world directory", error)
            }
            Log.i(TAG, "RKB RUST native version ${RkbNative.nativeVersion()}")
            Log.i(TAG, "Minecraft save directory: ${worldDir.absolutePath}")

            val rkbView = RkbView()
            val status = TextView(this).apply {
                setTextColor(Color.WHITE)
                textSize = 11f
                setPadding(18, 14, 18, 14)
                setBackgroundColor(Color.argb(150, 8, 12, 20))
                setShadowLayer(2f, 0f, 1f, Color.BLACK)
                text = overlayText("Starting Rust engine…")
            }
            val root = FrameLayout(this)
            root.addView(
                rkbView,
                FrameLayout.LayoutParams(
                    FrameLayout.LayoutParams.MATCH_PARENT,
                    FrameLayout.LayoutParams.MATCH_PARENT,
                ),
            )
            root.addView(
                status,
                FrameLayout.LayoutParams(
                    FrameLayout.LayoutParams.MATCH_PARENT,
                    FrameLayout.LayoutParams.WRAP_CONTENT,
                ).apply { gravity = Gravity.TOP },
            )
            view = rkbView
            overlay = status
            setContentView(root)
        } catch (error: Throwable) {
            Log.e(TAG, "startup failed", error)
            showStartupError(error.message ?: error.javaClass.simpleName)
        } finally {
            hideSystemUi()
        }
    }

    private fun overlayText(stats: String): String = buildString {
        append(stats)
        append("\nLeft stick: move · Drag: look · JUMP / FLY")
        append("\nSave: ")
        append(worldDir.absolutePath)
    }

    private fun showStartupError(detail: String) {
        val message = TextView(this).apply {
            gravity = Gravity.CENTER
            text = getString(R.string.startup_error, detail)
            textSize = 16f
            setPadding(48, 48, 48, 48)
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
        try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                // PhoneWindow.getInsetsController() dereferences the decor
                // view, which does not exist until the content view has been
                // installed. Go through the (nullable) view controller instead
                // of calling window.insetsController during startup.
                val controller = window.decorView.windowInsetsController ?: return
                controller.hide(WindowInsets.Type.statusBars() or WindowInsets.Type.navigationBars())
                controller.systemBarsBehavior =
                    WindowInsetsController.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
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
        } catch (error: RuntimeException) {
            // Immersive mode is cosmetic; never let it abort startup.
            Log.w(TAG, "could not hide system UI", error)
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
        private var surfaceWidth = 0

        @Volatile
        private var surfaceHeight = 0

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
                        engineHandle = RkbNative.nativeCreate(
                            bufferWidth,
                            bufferHeight,
                            worldDir.absolutePath,
                        )
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
                        if (
                            RkbNative.nativeRenderFrame(
                                engineHandle,
                                buffer,
                                bufferWidth,
                                bufferHeight,
                                delta,
                            )
                        ) {
                            buffer.rewind()
                            target.copyPixelsFromBuffer(buffer)
                            canvas.drawBitmap(
                                target,
                                null,
                                Rect(0, 0, width, height),
                                null,
                            )
                            frameCount++
                            if (frameCount == 1L) {
                                Log.i(TAG, "first frame presented (${width}x$height)")
                            }
                            if (frameCount == 1L || frameCount % 30L == 0L) {
                                val stats = RkbNative.nativeStats(engineHandle)
                                if (frameCount == 1L || frameCount % 60L == 0L) {
                                    Log.i(TAG, stats)
                                }
                                updateOverlay(stats)
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

        private fun ensureBuffers(surfaceWidth: Int, surfaceHeight: Int) {
            this.surfaceWidth = surfaceWidth
            this.surfaceHeight = surfaceHeight
            // The software rasterizer is fill-rate bound; render below native
            // resolution and let the compositor scale the bitmap up.
            val width = (surfaceWidth * RENDER_SCALE).toInt().coerceAtLeast(240)
            val height = (surfaceHeight * RENDER_SCALE).toInt().coerceAtLeast(135)
            if (width == bufferWidth && height == bufferHeight && frameBuffer != null) {
                return
            }
            val bytes = width.toLong() * height.toLong() * 4L
            check(bytes in 4..Int.MAX_VALUE.toLong()) {
                "invalid render size ${width}x$height"
            }
            frameBuffer = ByteBuffer.allocateDirect(bytes.toInt()).order(ByteOrder.nativeOrder())
            val oldBitmap = bitmap
            bitmap = Bitmap.createBitmap(width, height, Bitmap.Config.ARGB_8888)
            oldBitmap?.recycle()
            bufferWidth = width
            bufferHeight = height
        }

        private fun updateOverlay(stats: String) {
            val current = overlay ?: return
            runOnUiThread {
                if (!isFinishing && !isDestroyed) {
                    current.text = overlayText(stats)
                }
            }
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
                // Touch arrives in surface pixels; native input is laid out in
                // render-buffer pixels.
                val x = if (surfaceWidth > 0) {
                    sample.x * bufferWidth / surfaceWidth
                } else {
                    sample.x
                }
                val y = if (surfaceHeight > 0) {
                    sample.y * bufferHeight / surfaceHeight
                } else {
                    sample.y
                }
                RkbNative.nativeTouch(
                    handle,
                    sample.action,
                    sample.pointerId,
                    x,
                    y,
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
        const val RENDER_SCALE = 0.6f
    }
}
