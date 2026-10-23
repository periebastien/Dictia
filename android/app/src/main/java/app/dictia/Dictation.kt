package app.dictia

/**
 * Enregistrement en cours, partagé entre la bulle (DictiaService) et le
 * service de premier plan qui garde l'accès au micro (MicService).
 */
object Dictation {
    private var recorder: Recorder? = null
    private var active = false

    /** Appelé (thread principal) si le micro ne démarre pas. */
    var onError: ((String) -> Unit)? = null

    @Synchronized
    fun begin() {
        active = true
    }

    /** Démarre la capture si la dictée est toujours en cours. */
    @Synchronized
    fun startRecorder() {
        if (!active || recorder != null) return
        try {
            recorder = Recorder().also { it.start() }
        } catch (e: Exception) {
            active = false
            onError?.invoke("Micro : ${e.message}")
        }
    }

    /** Termine la dictée et renvoie l'audio capturé (vide si rien). */
    @Synchronized
    fun end(): ByteArray {
        active = false
        val pcm = recorder?.stop() ?: ByteArray(0)
        recorder = null
        return pcm
    }
}
