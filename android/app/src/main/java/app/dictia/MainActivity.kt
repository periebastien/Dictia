package app.dictia

import android.Manifest
import android.annotation.SuppressLint
import android.app.Activity
import android.content.ComponentName
import android.content.Intent
import android.content.pm.PackageManager
import android.graphics.Color
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.provider.Settings
import android.view.WindowInsets
import android.webkit.JavascriptInterface
import android.webkit.WebResourceRequest
import android.webkit.WebView
import android.webkit.WebViewClient
import android.widget.FrameLayout
import org.json.JSONArray
import org.json.JSONObject

/**
 * Écran de Dictia (réglages et vocabulaire). L'interface est une page locale
 * (`assets/settings.html`, même charte que la version Windows) reliée au code
 * par l'objet JavaScript `Android`.
 */
class MainActivity : Activity() {
    private lateinit var web: WebView

    @SuppressLint("SetJavaScriptEnabled")
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val background = Color.parseColor("#1B1B21")
        window.decorView.setBackgroundColor(background)
        val root = FrameLayout(this).apply { setBackgroundColor(background) }
        web = WebView(this).apply {
            setBackgroundColor(background)
            settings.javaScriptEnabled = true
            settings.allowFileAccess = false
            settings.allowContentAccess = false
            // Page locale uniquement : aucun lien ne s'ouvre dans l'application.
            webViewClient = object : WebViewClient() {
                override fun shouldOverrideUrlLoading(view: WebView, request: WebResourceRequest) = true
            }
            addJavascriptInterface(Bridge(), "Android")
            loadUrl("file:///android_asset/settings.html")
        }
        root.addView(web)
        // Android 15 dessine sous les barres système : on garde le contenu
        // entre la barre d'état, la barre de navigation et le clavier.
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            window.setDecorFitsSystemWindows(false)
            root.setOnApplyWindowInsetsListener { view, insets ->
                val bars = insets.getInsets(WindowInsets.Type.systemBars() or WindowInsets.Type.ime())
                view.setPadding(bars.left, bars.top, bars.right, bars.bottom)
                WindowInsets.CONSUMED
            }
        }
        setContentView(root)
    }

    override fun onResume() {
        super.onResume()
        pushState()
    }

    override fun onRequestPermissionsResult(code: Int, permissions: Array<String>, results: IntArray) {
        super.onRequestPermissionsResult(code, permissions, results)
        pushState()
    }

    @Suppress("OVERRIDE_DEPRECATION")
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        super.onActivityResult(requestCode, resultCode, data)
        val uri = data?.data ?: return
        if (resultCode != RESULT_OK) return
        when (requestCode) {
            IMPORT -> importVocab(uri)
            EXPORT -> exportVocab(uri)
        }
    }

    private fun importVocab(uri: Uri) {
        try {
            val text = contentResolver.openInputStream(uri)!!.use { it.readBytes().toString(Charsets.UTF_8) }
            val vocab = Vocab.fromToml(text)
            Prefs.saveVocab(this, vocab)
            pushState()
            toast("Importé : ${vocab.cleanTerms.size} terme(s), ${vocab.replacements.size} remplacement(s).")
        } catch (e: Exception) {
            toast("Fichier illisible : ${e.message}")
        }
    }

    private fun exportVocab(uri: Uri) {
        try {
            contentResolver.openOutputStream(uri, "wt")!!.use {
                it.write(Prefs.loadVocab(this).toToml().toByteArray(Charsets.UTF_8))
            }
            toast("Vocabulaire exporté.")
        } catch (e: Exception) {
            toast("Export impossible : ${e.message}")
        }
    }

    private fun pushState() {
        if (!::web.isInitialized) return
        val json = JSONObject.quote(state().toString())
        web.evaluateJavascript("window.onState && onState(JSON.parse($json))", null)
    }

    private fun toast(message: String) {
        val json = JSONObject.quote(message)
        web.evaluateJavascript("window.toast && toast($json)", null)
    }

    /** État affiché par la page. La clé API n'est jamais renvoyée en clair. */
    private fun state(): JSONObject {
        val prefs = Prefs.load(this)
        val mic = checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED
        fun keyHint(key: String) = when {
            key.isBlank() -> ""
            key.length <= 8 -> "••••••"
            else -> "••••••" + key.trim().takeLast(4)
        }
        val models = JSONObject()
        for (provider in listOf(Prefs.OPENAI, Prefs.MISTRAL)) {
            for (engine in listOf(Prefs.STANDARD, Prefs.REALTIME)) {
                models.put("$provider/$engine", JSONArray(Prefs.models(provider, engine)))
            }
        }
        return JSONObject()
            .put("version", packageManager.getPackageInfo(packageName, 0).versionName)
            .put("mic", mic)
            .put("bubble", accessibilityEnabled())
            .put("provider", prefs.provider)
            .put("engine", prefs.engine)
            .put("keyHint", JSONObject().put(Prefs.OPENAI, keyHint(prefs.openAiKey)).put(Prefs.MISTRAL, keyHint(prefs.mistralKey)))
            .put("models", models)
            .put(
                "selected",
                JSONObject()
                    .put("${Prefs.OPENAI}/${Prefs.STANDARD}", prefs.openAiModel)
                    .put("${Prefs.OPENAI}/${Prefs.REALTIME}", prefs.openAiRealtimeModel)
                    .put("${Prefs.MISTRAL}/${Prefs.STANDARD}", prefs.mistralModel)
                    .put("${Prefs.MISTRAL}/${Prefs.REALTIME}", prefs.mistralRealtimeModel),
            )
            .put("language", prefs.language)
            .put("languages", JSONArray(Prefs.LANGUAGES.map { JSONArray(listOf(it.first, it.second)) }))
            .put("shortWords", prefs.shortTextWords)
            .put("shortChoices", JSONArray(Prefs.SHORT_TEXT_WORDS))
            .put("vocab", Prefs.loadVocab(this).toJson())
    }

    private fun accessibilityEnabled(): Boolean {
        val enabled = Settings.Secure.getString(
            contentResolver, Settings.Secure.ENABLED_ACCESSIBILITY_SERVICES,
        ).orEmpty()
        val me = ComponentName(this, DictiaService::class.java)
        return enabled.split(':').any { ComponentName.unflattenFromString(it) == me }
    }

    private fun change(kind: String, value: String) {
        var p = Prefs.load(this)
        p = when (kind) {
            "provider" -> p.copy(provider = if (value == Prefs.MISTRAL) Prefs.MISTRAL else Prefs.OPENAI)
            "engine" -> p.copy(engine = if (value == Prefs.REALTIME) Prefs.REALTIME else Prefs.STANDARD)
            "key" -> if (p.isMistral) p.copy(mistralKey = value) else p.copy(openAiKey = value)
            "model" -> when {
                value.isBlank() -> p
                p.isMistral && p.realtime -> p.copy(mistralRealtimeModel = value)
                p.isMistral -> p.copy(mistralModel = value)
                p.realtime -> p.copy(openAiRealtimeModel = value)
                else -> p.copy(openAiModel = value)
            }
            "language" -> p.copy(language = value)
            "short_words" -> p.copy(shortTextWords = value.toIntOrNull()?.coerceIn(0, 20) ?: p.shortTextWords)
            else -> return
        }
        p.save(this)
    }

    private fun action(name: String) {
        when (name) {
            "mic" -> {
                val wanted = mutableListOf(Manifest.permission.RECORD_AUDIO)
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                    wanted += Manifest.permission.POST_NOTIFICATIONS
                }
                requestPermissions(wanted.toTypedArray(), 1)
            }
            "accessibility" -> startActivity(Intent(Settings.ACTION_ACCESSIBILITY_SETTINGS))
            "app_settings" -> startActivity(
                Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.fromParts("package", packageName, null)),
            )
            "import" -> startActivityForResult(
                Intent(Intent.ACTION_OPEN_DOCUMENT).addCategory(Intent.CATEGORY_OPENABLE).setType("*/*"),
                IMPORT,
            )
            "export" -> startActivityForResult(
                Intent(Intent.ACTION_CREATE_DOCUMENT).addCategory(Intent.CATEGORY_OPENABLE)
                    .setType("application/octet-stream")
                    .putExtra(Intent.EXTRA_TITLE, "vocab.toml"),
                EXPORT,
            )
        }
    }

    /** Appels de la page (thread du WebView → thread principal). */
    private inner class Bridge {
        @JavascriptInterface
        fun ready() = runOnUiThread { pushState() }

        @JavascriptInterface
        fun change(kind: String, value: String) = runOnUiThread {
            this@MainActivity.change(kind, value)
            pushState()
        }

        @JavascriptInterface
        fun vocab(json: String) = runOnUiThread {
            runCatching { Prefs.saveVocab(this@MainActivity, Vocab.fromJson(JSONObject(json))) }
        }

        @JavascriptInterface
        fun action(name: String) = runOnUiThread { this@MainActivity.action(name) }
    }

    private companion object {
        const val IMPORT = 1
        const val EXPORT = 2
    }
}
