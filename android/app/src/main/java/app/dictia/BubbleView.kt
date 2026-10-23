package app.dictia

import android.content.Context
import android.graphics.Canvas
import android.graphics.Paint
import android.graphics.Path
import android.graphics.RectF
import android.os.SystemClock
import android.view.View
import kotlin.math.PI
import kotlin.math.abs
import kotlin.math.max
import kotlin.math.sin

/**
 * Bulle flottante, dessinée comme l'icône et l'indicateur de la version Windows :
 * - **repos** : carré ambre aux coins arrondis avec le micro ;
 * - **écoute** : point rouge qui pulse et barres qui suivent le volume du micro ;
 * - **transcription** : trois points bleus qui ondulent.
 */
class BubbleView(context: Context) : View(context) {
    enum class Mode { IDLE, LISTENING, THINKING }

    var mode = Mode.IDLE
        set(value) {
            field = value
            started = SystemClock.uptimeMillis()
            invalidate()
        }

    /** Volume du micro (0 à 1) lu à chaque image pendant l'écoute. */
    var level: () -> Float = { 0f }

    private var started = SystemClock.uptimeMillis()
    private var smoothLevel = 0f
    private val paint = Paint(Paint.ANTI_ALIAS_FLAG)
    private val rect = RectF()

    /** Micro du logo sur une grille de 24 unités (voir assets/generate_icon.py). */
    private val capsule = Path().apply { addRoundRect(RectF(8f, 1.2f, 16f, 15.4f), 4f, 4f, Path.Direction.CW) }
    private val stand = Path().apply {
        moveTo(5f, 9.5f)
        lineTo(5f, 12f)
        arcTo(RectF(5f, 5f, 19f, 19f), 180f, -180f)
        lineTo(19f, 9.5f)
        moveTo(12f, 19f)
        lineTo(12f, 22.4f)
    }

    override fun onDraw(canvas: Canvas) {
        val s = width.toFloat()
        val t = (SystemClock.uptimeMillis() - started) / 1000f
        rect.set(0f, 0f, s, s)
        paint.style = Paint.Style.FILL
        when (mode) {
            Mode.IDLE -> {
                paint.color = AMBER
                canvas.drawRoundRect(rect, s * 0.24f, s * 0.24f, paint)
                drawMic(canvas, s)
            }
            Mode.LISTENING -> {
                paint.color = BACKGROUND
                canvas.drawRoundRect(rect, s * 0.3f, s * 0.3f, paint)
                drawListening(canvas, s, t)
            }
            Mode.THINKING -> {
                paint.color = BACKGROUND
                canvas.drawRoundRect(rect, s * 0.3f, s * 0.3f, paint)
                drawThinking(canvas, s, t)
            }
        }
        if (mode != Mode.IDLE) postInvalidateOnAnimation()
    }

    private fun drawMic(canvas: Canvas, s: Float) {
        canvas.save()
        canvas.translate(s * 0.19f, s * 0.19f)
        canvas.scale(s * 0.62f / 24f, s * 0.62f / 24f)
        paint.color = INK
        canvas.drawPath(capsule, paint)
        paint.style = Paint.Style.STROKE
        paint.strokeWidth = 2.6f
        paint.strokeCap = Paint.Cap.ROUND
        canvas.drawPath(stand, paint)
        paint.style = Paint.Style.FILL
        canvas.restore()
    }

    private fun drawListening(canvas: Canvas, s: Float, t: Float) {
        val cx = s * 0.3f
        val cy = s / 2
        val dot = s * 0.1f
        // Onde qui s'élargit et s'efface autour du point rouge.
        val phase = (t / 1.2f) % 1f
        paint.color = RED
        paint.alpha = (115 * (1 - phase)).toInt()
        canvas.drawCircle(cx, cy, dot + s * 0.12f * phase, paint)
        paint.alpha = 255
        canvas.drawCircle(cx, cy, dot, paint)

        // Barres de volume, légèrement désynchronisées entre elles.
        smoothLevel += (level().coerceIn(0f, 1f) - smoothLevel) * 0.3f
        paint.color = WHITE
        val barW = s * 0.07f
        for (i in 0 until 3) {
            val wobble = 0.65f + 0.35f * abs(sin(t * 9f + i * 1.3f))
            val h = s * 0.1f + s * 0.45f * smoothLevel * wobble
            val x = s * 0.52f + i * s * 0.13f
            rect.set(x, cy - h / 2, x + barW, cy + h / 2)
            canvas.drawRoundRect(rect, barW / 2, barW / 2, paint)
        }
    }

    private fun drawThinking(canvas: Canvas, s: Float, t: Float) {
        paint.color = BLUE
        for (i in 0 until 3) {
            val wave = max(0f, sin(t * 2 * PI.toFloat() * 1.4f - i * 0.9f))
            paint.alpha = (255 * (0.7f + 0.3f * wave)).toInt()
            canvas.drawCircle(s * (0.25f + i * 0.25f), s / 2 - s * 0.12f * wave, s * 0.085f, paint)
        }
        paint.alpha = 255
    }

    private companion object {
        const val AMBER = 0xFFF59E0B.toInt()
        const val INK = 0xFF0E0E12.toInt()
        const val BACKGROUND = 0xE618181B.toInt()
        const val RED = 0xFFF87171.toInt()
        const val BLUE = 0xFF60A5FA.toInt()
        const val WHITE = 0xFFF2F2F2.toInt()
    }
}
