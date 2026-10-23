package app.dictia

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.Service
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder

/**
 * Service de premier plan « micro » : Android exige une notification visible
 * pour enregistrer pendant qu'une autre application est à l'écran.
 */
class MicService : Service() {
    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val manager = getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(
            NotificationChannel(CHANNEL, "Dictée en cours", NotificationManager.IMPORTANCE_LOW),
        )
        val notification = Notification.Builder(this, CHANNEL)
            .setSmallIcon(R.drawable.ic_mic)
            .setContentTitle("Dictia écoute")
            .setContentText("Touchez la bulle pour terminer la dictée.")
            .setOngoing(true)
            .build()
        try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                startForeground(ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE)
            } else {
                startForeground(ID, notification)
            }
        } catch (e: Exception) {
            // On tente quand même l'enregistrement : le service d'accessibilité
            // peut suffire à garder l'accès au micro selon les versions d'Android.
        }
        Dictation.startRecorder()
        return START_NOT_STICKY
    }

    private companion object {
        const val CHANNEL = "dictia_micro"
        const val ID = 1
    }
}
