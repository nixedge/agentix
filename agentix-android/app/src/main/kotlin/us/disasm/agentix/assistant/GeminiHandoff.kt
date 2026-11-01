package us.disasm.agentix.assistant

import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager

internal object GeminiHandoff {
    fun open(context: Context, question: String): Boolean {
        for (action in listOf(Intent.ACTION_SEND, Intent.ACTION_PROCESS_TEXT)) {
            for (packageName in listOf("com.google.android.apps.bard", "com.google.android.googlequicksearchbox")) {
                val intent = Intent(action).setType("text/plain").setPackage(packageName)
                if (action == Intent.ACTION_SEND) intent.putExtra(Intent.EXTRA_TEXT, question)
                else intent.putExtra(Intent.EXTRA_PROCESS_TEXT, question).putExtra(Intent.EXTRA_PROCESS_TEXT_READONLY, true)
                @Suppress("DEPRECATION")
                val handlers = context.packageManager.queryIntentActivities(intent, PackageManager.MATCH_ALL)
                val handler = handlers.sortedBy { it.activityInfo.name }.firstOrNull { candidate ->
                    val info = candidate.activityInfo
                    info.packageName == packageName && info.enabled && info.exported && info.permission.isNullOrEmpty() &&
                        (packageName == "com.google.android.apps.bard" || candidate.loadLabel(context.packageManager).toString().contains("Gemini", ignoreCase = true))
                } ?: continue
                try {
                    context.startActivity(intent.setComponent(ComponentName(packageName, handler.activityInfo.name)))
                    return true
                } catch (_: android.content.ActivityNotFoundException) { /* Handler disappeared; try another registered Gemini handler. */ }
                catch (_: SecurityException) { /* An engine may expose an unusable handler. */ }
            }
        }
        return false
    }
}
