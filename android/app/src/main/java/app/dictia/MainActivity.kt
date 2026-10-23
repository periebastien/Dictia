package app.dictia

import android.Manifest
import android.app.Activity
import android.content.ComponentName
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import android.provider.Settings
import android.text.InputType
import android.view.View
import android.widget.ArrayAdapter
import android.widget.Button
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.RadioButton
import android.widget.RadioGroup
import android.widget.ScrollView
import android.widget.Spinner
import android.widget.TextView
import android.widget.Toast

/** Écran de réglages : autorisations, fournisseur, clé API, modèle, langue. */
class MainActivity : Activity() {
    private lateinit var status: TextView
    private lateinit var micButton: Button
    private lateinit var a11yButton: Button
    private lateinit var openAi: RadioButton
    private lateinit var mistral: RadioButton
    private lateinit var keyField: EditText
    private lateinit var modelField: EditText
    private lateinit var languageField: EditText
    private lateinit var shortWords: Spinner

    /** Valeurs en cours d'édition (la clé et le modèle dépendent du fournisseur). */
    private lateinit var draft: Prefs

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        draft = Prefs.load(this)
        val pad = dp(16)
        val column = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(pad, pad, pad, pad)
        }

        column.addView(title("Mise en route"))
        status = TextView(this).apply { textSize = 15f }
        column.addView(status)
        micButton = Button(this).apply {
            text = "1. Autoriser le micro"
            setOnClickListener { requestPermissions() }
        }
        column.addView(micButton)
        a11yButton = Button(this).apply {
            text = "2. Activer la bulle Dictia"
            setOnClickListener { startActivity(Intent(Settings.ACTION_ACCESSIBILITY_SETTINGS)) }
        }
        column.addView(a11yButton)
        column.addView(hint(
            "Dans Accessibilité, ouvrez « Applications installées » (ou « Services ») › Dictia " +
                "et activez-le. Si l'interrupteur est grisé : Paramètres › Applications › " +
                "Dictia › menu ⋮ › « Autoriser les paramètres restreints », puis recommencez.",
        ))

        column.addView(title("Transcription"))
        openAi = RadioButton(this).apply { text = "OpenAI"; id = View.generateViewId() }
        mistral = RadioButton(this).apply { text = "Mistral"; id = View.generateViewId() }
        val providers = RadioGroup(this).apply {
            orientation = RadioGroup.HORIZONTAL
            addView(openAi)
            addView(mistral)
        }
        column.addView(providers)
        column.addView(label("Clé API"))
        keyField = EditText(this).apply {
            inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_PASSWORD
            isSingleLine = true
        }
        column.addView(keyField)
        column.addView(label("Modèle"))
        modelField = EditText(this).apply { isSingleLine = true }
        column.addView(modelField)
        column.addView(label("Langue (fr, en, es… ou auto)"))
        languageField = EditText(this).apply { isSingleLine = true; setText(draft.language) }
        column.addView(languageField)

        column.addView(label("Textes courts sans ponctuation"))
        shortWords = Spinner(this).apply {
            adapter = ArrayAdapter(
                this@MainActivity,
                android.R.layout.simple_spinner_dropdown_item,
                SHORT_TEXT_WORDS.map { if (it == 0) "Non" else "Jusqu'à $it mot" + if (it > 1) "s" else "" },
            )
            setSelection(SHORT_TEXT_WORDS.indexOf(draft.shortTextWords).coerceAtLeast(0))
        }
        column.addView(shortWords)

        column.addView(Button(this).apply {
            text = "Enregistrer"
            setOnClickListener { save() }
        })
        column.addView(hint(
            "Utilisation : touchez la bulle pour parler, touchez-la de nouveau pour écrire le " +
                "texte là où se trouve le curseur. Faites-la glisser pour la déplacer ; un appui " +
                "long ouvre ces réglages.",
        ))

        if (draft.provider == Prefs.MISTRAL) mistral.isChecked = true else openAi.isChecked = true
        showProvider()
        providers.setOnCheckedChangeListener { _, _ ->
            keepProviderFields()
            draft = draft.copy(provider = if (mistral.isChecked) Prefs.MISTRAL else Prefs.OPENAI)
            showProvider()
        }

        setContentView(ScrollView(this).apply { addView(column) })
    }

    override fun onResume() {
        super.onResume()
        val mic = checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED
        val a11y = accessibilityEnabled()
        micButton.visibility = if (mic) View.GONE else View.VISIBLE
        a11yButton.text = if (a11y) "Réglages d'accessibilité" else "2. Activer la bulle Dictia"
        status.text = buildString {
            append(if (mic) "✅ Micro autorisé\n" else "❌ Micro non autorisé\n")
            append(if (a11y) "✅ Bulle active" else "❌ Bulle inactive")
        }
    }

    private fun requestPermissions() {
        val wanted = mutableListOf(Manifest.permission.RECORD_AUDIO)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            wanted += Manifest.permission.POST_NOTIFICATIONS
        }
        requestPermissions(wanted.toTypedArray(), 1)
    }

    override fun onRequestPermissionsResult(code: Int, permissions: Array<String>, results: IntArray) {
        super.onRequestPermissionsResult(code, permissions, results)
        onResume()
    }

    private fun accessibilityEnabled(): Boolean {
        val enabled = Settings.Secure.getString(
            contentResolver, Settings.Secure.ENABLED_ACCESSIBILITY_SERVICES,
        ).orEmpty()
        val me = ComponentName(this, DictiaService::class.java)
        return enabled.split(':').any { ComponentName.unflattenFromString(it) == me }
    }

    private fun showProvider() {
        val isMistral = draft.provider == Prefs.MISTRAL
        keyField.setText(if (isMistral) draft.mistralKey else draft.openAiKey)
        modelField.setText(if (isMistral) draft.mistralModel else draft.openAiModel)
    }

    private fun keepProviderFields() {
        val key = keyField.text.toString().trim()
        val model = modelField.text.toString().trim()
        draft = if (draft.provider == Prefs.MISTRAL) {
            draft.copy(mistralKey = key, mistralModel = model.ifEmpty { Prefs.MISTRAL_MODEL })
        } else {
            draft.copy(openAiKey = key, openAiModel = model.ifEmpty { Prefs.OPENAI_MODEL })
        }
    }

    private fun save() {
        keepProviderFields()
        draft = draft.copy(
            language = languageField.text.toString().trim().ifEmpty { "fr" },
            shortTextWords = SHORT_TEXT_WORDS[shortWords.selectedItemPosition.coerceAtLeast(0)],
        )
        draft.save(this)
        Toast.makeText(this, "Réglages enregistrés", Toast.LENGTH_SHORT).show()
    }

    private fun title(text: String) = TextView(this).apply {
        this.text = text
        textSize = 20f
        setPadding(0, dp(16), 0, dp(8))
    }

    private fun label(text: String) = TextView(this).apply {
        this.text = text
        setPadding(0, dp(12), 0, 0)
    }

    private fun hint(text: String) = TextView(this).apply {
        this.text = text
        textSize = 13f
        alpha = 0.7f
        setPadding(0, dp(4), 0, dp(4))
    }

    private fun dp(value: Int) = (value * resources.displayMetrics.density).toInt()

    private companion object {
        val SHORT_TEXT_WORDS = listOf(0, 1, 2, 3, 4, 5, 6)
    }
}
