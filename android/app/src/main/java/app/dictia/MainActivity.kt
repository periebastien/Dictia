package app.dictia

import android.Manifest
import android.app.Activity
import android.content.ComponentName
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import android.provider.Settings
import android.text.Editable
import android.text.InputType
import android.text.TextWatcher
import android.view.View
import android.widget.AdapterView
import android.widget.ArrayAdapter
import android.widget.Button
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.RadioButton
import android.widget.RadioGroup
import android.widget.ScrollView
import android.widget.Spinner
import android.widget.TextView

/**
 * Écran de réglages : autorisations, fournisseur, clé API, modèle, langue.
 * Chaque modification est enregistrée aussitôt.
 */
class MainActivity : Activity() {
    private lateinit var status: TextView
    private lateinit var micButton: Button
    private lateinit var a11yButton: Button
    private lateinit var mistral: RadioButton
    private lateinit var keyField: EditText
    private lateinit var modelSpinner: Spinner
    private lateinit var customModel: EditText

    /** Réglages en cours (la clé et le modèle dépendent du fournisseur). */
    private lateinit var prefs: Prefs

    /** Évite d'enregistrer pendant qu'on remplit les champs par programme. */
    private var filling = false

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        prefs = Prefs.load(this)
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
        val openAi = RadioButton(this).apply { text = "OpenAI"; id = View.generateViewId() }
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
            onChange { key ->
                prefs = if (prefs.provider == Prefs.MISTRAL) {
                    prefs.copy(mistralKey = key)
                } else {
                    prefs.copy(openAiKey = key)
                }
            }
        }
        column.addView(keyField)

        column.addView(label("Modèle"))
        modelSpinner = Spinner(this)
        column.addView(modelSpinner)
        customModel = EditText(this).apply {
            hint = "Nom du modèle"
            isSingleLine = true
            visibility = View.GONE
            onChange { name -> if (name.isNotEmpty()) setModel(name) }
        }
        column.addView(customModel)

        column.addView(label("Langue"))
        val languages = Prefs.LANGUAGES.toMutableList()
        if (languages.none { it.first == prefs.language }) {
            languages += prefs.language to prefs.language
        }
        column.addView(spinner(
            languages.map { it.second },
            languages.indexOfFirst { it.first == prefs.language },
        ) { i -> prefs = prefs.copy(language = languages[i].first) })

        column.addView(label("Textes courts sans ponctuation"))
        column.addView(spinner(
            SHORT_TEXT_WORDS.map { if (it == 0) "Non" else "Jusqu'à $it mot" + if (it > 1) "s" else "" },
            SHORT_TEXT_WORDS.indexOf(prefs.shortTextWords),
        ) { i -> prefs = prefs.copy(shortTextWords = SHORT_TEXT_WORDS[i]) })

        column.addView(hint(
            "Les réglages sont enregistrés automatiquement.\n\nUtilisation : la bulle apparaît " +
                "quand un champ de saisie a le curseur. Touchez-la pour parler, touchez-la de " +
                "nouveau pour écrire le texte. Faites-la glisser pour la déplacer ; un appui " +
                "long ouvre ces réglages.",
        ))

        if (prefs.provider == Prefs.MISTRAL) mistral.isChecked = true else openAi.isChecked = true
        showProvider()
        providers.setOnCheckedChangeListener { _, _ ->
            prefs = prefs.copy(provider = if (mistral.isChecked) Prefs.MISTRAL else Prefs.OPENAI)
            prefs.save(this)
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

    /** Remplit la clé et la liste des modèles du fournisseur choisi. */
    private fun showProvider() {
        filling = true
        keyField.setText(if (prefs.provider == Prefs.MISTRAL) prefs.mistralKey else prefs.openAiKey)
        val models = Prefs.suggestedModels(prefs.provider)
        val current = prefs.model
        val custom = current !in models
        val labels = models + OTHER
        modelSpinner.adapter = ArrayAdapter(this, android.R.layout.simple_spinner_dropdown_item, labels)
        modelSpinner.setSelection(if (custom) labels.lastIndex else models.indexOf(current))
        customModel.setText(if (custom) current else "")
        customModel.visibility = if (custom) View.VISIBLE else View.GONE
        modelSpinner.onItemSelectedListener = object : AdapterView.OnItemSelectedListener {
            override fun onItemSelected(parent: AdapterView<*>?, view: View?, i: Int, id: Long) {
                if (filling) return
                if (i < models.size) {
                    customModel.visibility = View.GONE
                    setModel(models[i])
                } else {
                    customModel.visibility = View.VISIBLE
                    customModel.requestFocus()
                }
            }

            override fun onNothingSelected(parent: AdapterView<*>?) = Unit
        }
        // Le Spinner signale sa sélection initiale après coup : on attend.
        modelSpinner.post { filling = false }
    }

    private fun setModel(name: String) {
        prefs = if (prefs.provider == Prefs.MISTRAL) {
            prefs.copy(mistralModel = name)
        } else {
            prefs.copy(openAiModel = name)
        }
        prefs.save(this)
    }

    private fun spinner(labels: List<String>, selected: Int, onSelect: (Int) -> Unit) =
        Spinner(this).apply {
            adapter = ArrayAdapter(this@MainActivity, android.R.layout.simple_spinner_dropdown_item, labels)
            setSelection(selected.coerceAtLeast(0))
            onItemSelectedListener = object : AdapterView.OnItemSelectedListener {
                override fun onItemSelected(parent: AdapterView<*>?, view: View?, i: Int, id: Long) {
                    onSelect(i)
                    prefs.save(this@MainActivity)
                }

                override fun onNothingSelected(parent: AdapterView<*>?) = Unit
            }
        }

    /** Enregistre à chaque frappe (hors remplissage par programme). */
    private fun EditText.onChange(apply: (String) -> Unit) {
        addTextChangedListener(object : TextWatcher {
            override fun beforeTextChanged(s: CharSequence?, start: Int, count: Int, after: Int) = Unit
            override fun onTextChanged(s: CharSequence?, start: Int, before: Int, count: Int) = Unit
            override fun afterTextChanged(s: Editable?) {
                if (filling) return
                apply(s.toString().trim())
                prefs.save(this@MainActivity)
            }
        })
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
        const val OTHER = "Autre…"
    }
}
