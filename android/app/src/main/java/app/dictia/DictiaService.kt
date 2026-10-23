package app.dictia

import android.Manifest
import android.accessibilityservice.AccessibilityService
import android.annotation.SuppressLint
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Intent
import android.content.pm.PackageManager
import android.graphics.PixelFormat
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.view.Gravity
import android.view.HapticFeedbackConstants
import android.view.MotionEvent
import android.view.View
import android.view.ViewConfiguration
import android.view.WindowManager
import android.view.accessibility.AccessibilityEvent
import android.view.accessibility.AccessibilityNodeInfo
import android.widget.Toast
import java.util.concurrent.Executors
import kotlin.math.abs

/**
 * Service d'accessibilité : affiche la bulle par-dessus les autres
 * applications et écrit le texte dicté dans le champ qui a le curseur.
 *
 * La bulle n'est pas focalisable : la toucher laisse le curseur (et le
 * clavier) dans l'application en cours. Elle n'apparaît que lorsqu'un champ
 * de saisie a le curseur (ou pendant une dictée).
 */
class DictiaService : AccessibilityService() {
    private enum class State { IDLE, RECORDING, PROCESSING }

    private val main = Handler(Looper.getMainLooper())
    private val worker = Executors.newSingleThreadExecutor()
    private lateinit var windowManager: WindowManager
    private var bubble: BubbleView? = null
    private var bubbleShown = true
    private lateinit var params: WindowManager.LayoutParams
    private var state = State.IDLE

    /** Dernier champ de saisie qui a eu le focus (secours si le focus est perdu). */
    private var lastEditable: AccessibilityNodeInfo? = null

    override fun onServiceConnected() {
        windowManager = getSystemService(WindowManager::class.java)
        Dictation.onError = { message ->
            setState(State.IDLE)
            stopService(Intent(this, MicService::class.java))
            toast(message)
        }
        showBubble()
    }

    override fun onAccessibilityEvent(event: AccessibilityEvent) {
        event.source?.takeIf { it.isEditable }?.let { lastEditable = it }
        // Regroupe les rafales d'événements avant de chercher le champ actif.
        main.removeCallbacks(refreshVisibility)
        main.postDelayed(refreshVisibility, 120)
    }

    private val refreshVisibility = Runnable {
        showBubbleIf(state != State.IDLE || currentEditable() != null)
    }

    override fun onInterrupt() = Unit

    override fun onDestroy() {
        Dictation.onError = null
        Dictation.end()
        main.removeCallbacks(refreshVisibility)
        bubble?.let { runCatching { windowManager.removeView(it) } }
        bubble = null
        worker.shutdown()
        super.onDestroy()
    }

    // --- Bulle -------------------------------------------------------------

    @SuppressLint("ClickableViewAccessibility")
    private fun showBubble() {
        val size = (40 * resources.displayMetrics.density).toInt()
        val (x, y) = Prefs.bubblePosition(this)
        params = WindowManager.LayoutParams(
            size,
            size,
            WindowManager.LayoutParams.TYPE_ACCESSIBILITY_OVERLAY,
            WindowManager.LayoutParams.FLAG_NOT_FOCUSABLE or
                WindowManager.LayoutParams.FLAG_LAYOUT_NO_LIMITS,
            PixelFormat.TRANSLUCENT,
        ).apply {
            gravity = Gravity.TOP or Gravity.START
            this.x = x
            this.y = y
        }
        val view = BubbleView(this).apply {
            contentDescription = "Dictia"
            level = { Dictation.level }
        }
        val slop = ViewConfiguration.get(this).scaledTouchSlop
        var downX = 0f
        var downY = 0f
        var startX = 0
        var startY = 0
        var moved = false
        val longPress = Runnable {
            if (!moved && state == State.IDLE) openSettings()
        }
        view.setOnTouchListener { v, e ->
            when (e.actionMasked) {
                MotionEvent.ACTION_DOWN -> {
                    downX = e.rawX
                    downY = e.rawY
                    startX = params.x
                    startY = params.y
                    moved = false
                    main.postDelayed(longPress, ViewConfiguration.getLongPressTimeout().toLong())
                }
                MotionEvent.ACTION_MOVE -> {
                    val dx = e.rawX - downX
                    val dy = e.rawY - downY
                    if (!moved && (abs(dx) > slop || abs(dy) > slop)) {
                        moved = true
                        main.removeCallbacks(longPress)
                    }
                    if (moved) {
                        params.x = startX + dx.toInt()
                        params.y = startY + dy.toInt()
                        windowManager.updateViewLayout(v, params)
                    }
                }
                MotionEvent.ACTION_UP -> {
                    main.removeCallbacks(longPress)
                    if (moved) {
                        Prefs.saveBubblePosition(this, params.x, params.y)
                    } else if (e.eventTime - e.downTime < ViewConfiguration.getLongPressTimeout()) {
                        v.performHapticFeedback(HapticFeedbackConstants.VIRTUAL_KEY)
                        onTap()
                    }
                }
                MotionEvent.ACTION_CANCEL -> main.removeCallbacks(longPress)
            }
            true
        }
        windowManager.addView(view, params)
        bubble = view
        setState(State.IDLE)
    }

    /** Affiche ou cache la bulle ; cachée, elle laisse passer les touchers. */
    private fun showBubbleIf(show: Boolean) {
        val view = bubble ?: return
        if (show == bubbleShown) return
        bubbleShown = show
        view.visibility = if (show) View.VISIBLE else View.GONE
        params.flags = if (show) {
            params.flags and WindowManager.LayoutParams.FLAG_NOT_TOUCHABLE.inv()
        } else {
            params.flags or WindowManager.LayoutParams.FLAG_NOT_TOUCHABLE
        }
        windowManager.updateViewLayout(view, params)
    }

    private fun setState(newState: State) {
        state = newState
        bubble?.mode = when (newState) {
            State.IDLE -> BubbleView.Mode.IDLE
            State.RECORDING -> BubbleView.Mode.LISTENING
            State.PROCESSING -> BubbleView.Mode.THINKING
        }
        if (newState == State.IDLE) refreshVisibility.run()
    }

    // --- Dictée ------------------------------------------------------------

    private fun onTap() {
        when (state) {
            State.IDLE -> startDictation()
            State.RECORDING -> finishDictation()
            State.PROCESSING -> Unit
        }
    }

    private fun startDictation() {
        if (checkSelfPermission(Manifest.permission.RECORD_AUDIO) != PackageManager.PERMISSION_GRANTED) {
            toast("Autorisez le micro dans l'application Dictia.")
            openSettings()
            return
        }
        val prefs = Prefs.load(this)
        if (prefs.apiKey.isBlank()) {
            toast("Ajoutez votre clé API dans l'application Dictia.")
            openSettings()
            return
        }
        Dictation.begin()
        setState(State.RECORDING)
        try {
            startForegroundService(Intent(this, MicService::class.java))
        } catch (e: Exception) {
            // Service de premier plan refusé : enregistrement direct.
            Dictation.startRecorder()
        }
        worker.execute { Transcriber(prefs).warmUp() }
    }

    private fun finishDictation() {
        setState(State.PROCESSING)
        val pcm = Dictation.end()
        stopService(Intent(this, MicService::class.java))
        val prefs = Prefs.load(this)
        worker.execute {
            val result = runCatching {
                Audio.trimSilence(pcm)?.let { Transcriber(prefs).transcribe(Audio.wav(it)) }
            }
            main.post {
                setState(State.IDLE)
                result.onSuccess { raw ->
                    val text = raw?.trim().orEmpty()
                    if (text.isEmpty()) {
                        toast("Aucune parole détectée.")
                    } else {
                        insert(TextTools.stripShortPunctuation(text, prefs.shortTextWords))
                    }
                }.onFailure { toast("Erreur : ${it.message}") }
            }
        }
    }

    // --- Frappe du texte ---------------------------------------------------

    /** Champ de saisie qui a le curseur en ce moment, s'il y en a un. */
    private fun currentEditable(): AccessibilityNodeInfo? {
        rootInActiveWindow?.findFocus(AccessibilityNodeInfo.FOCUS_INPUT)
            ?.takeIf { it.isEditable }?.let { return it }
        for (window in windows) {
            window.root?.findFocus(AccessibilityNodeInfo.FOCUS_INPUT)
                ?.takeIf { it.isEditable }?.let { return it }
        }
        return null
    }

    private fun focusedEditable(): AccessibilityNodeInfo? =
        currentEditable() ?: lastEditable?.takeIf { it.refresh() && it.isEditable }

    private fun insert(text: String) {
        val node = focusedEditable()
        if (node != null && setText(node, text)) return
        // Secours : presse-papiers puis « coller » dans le champ.
        getSystemService(ClipboardManager::class.java)
            .setPrimaryClip(ClipData.newPlainText("Dictia", text))
        if (node?.performAction(AccessibilityNodeInfo.ACTION_PASTE) == true) return
        toast("Aucun champ de saisie trouvé : texte copié, collez-le.")
    }

    private fun setText(node: AccessibilityNodeInfo, text: String): Boolean {
        val existing = if (node.isShowingHintText) "" else node.text?.toString().orEmpty()
        val merged = TextTools.merge(existing, node.textSelectionStart, node.textSelectionEnd, text)
        val args = Bundle().apply {
            putCharSequence(AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE, merged.text)
        }
        if (!node.performAction(AccessibilityNodeInfo.ACTION_SET_TEXT, args)) return false
        node.performAction(
            AccessibilityNodeInfo.ACTION_SET_SELECTION,
            Bundle().apply {
                putInt(AccessibilityNodeInfo.ACTION_ARGUMENT_SELECTION_START_INT, merged.cursor)
                putInt(AccessibilityNodeInfo.ACTION_ARGUMENT_SELECTION_END_INT, merged.cursor)
            },
        )
        return true
    }

    // --- Divers ------------------------------------------------------------

    private fun openSettings() {
        startActivity(
            Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
        )
    }

    private fun toast(message: String) {
        Toast.makeText(this, message, Toast.LENGTH_LONG).show()
    }
}
