// Every case in tests/fixtures/day_verdicts.json, through the generated Kotlin bindings.

import org.ankiquest.rules.DayVerdict
import org.ankiquest.rules.PendingReview
import org.ankiquest.rules.ServerDay
import org.ankiquest.rules.ServerToday
import org.ankiquest.rules.StreakState
import org.ankiquest.rules.todayVerdict
import org.json.JSONObject
import java.io.File
import kotlin.system.exitProcess

private fun server(v: JSONObject): ServerDay {
    val today = v.getJSONObject("today")
    return ServerDay(
        streak = v.getInt("streak").toUInt(),
        streakState = StreakState.valueOf(v.getString("streak_state").uppercase()),
        dayEndsAt = v.getLong("day_ends_at"),
        lastReviewId = v.getLong("last_review_id"),
        today =
            ServerToday(
                startsAt = today.getLong("starts_at"),
                reviews = today.getInt("reviews").toUInt(),
                observed = today.optInt("observed", 0).toUInt(),
                timeMs = today.getLong("time_ms"),
                minutes = today.getInt("minutes").toUInt(),
                streakMinutes = today.getInt("streak_minutes").toUInt(),
            ),
    )
}

// The verdict in the fixture's own shape: null for a day that has ended.
private fun shape(verdict: DayVerdict): Any {
    if (verdict !is DayVerdict.Today) {
        return JSONObject.NULL
    }
    val v = verdict.verdict
    return JSONObject()
        .put("met", v.met)
        .put("streak_state", v.streakState?.name?.lowercase() ?: JSONObject.NULL)
        .put("streak", v.streak?.toLong() ?: JSONObject.NULL)
        .put("reviews", v.reviews.toLong())
        .put("observed", v.observed.toLong())
        .put("time_ms", v.timeMs)
        .put("minutes", v.minutes.toLong())
        .put("streak_minutes", v.streakMinutes.toLong())
        .put("unsent", v.unsent.toLong())
        .put("label", v.label.name.lowercase())
}

fun main(args: Array<String>) {
    val cases = JSONObject(File(args[0]).readText()).getJSONArray("cases")
    check(cases.length() > 0) { "no cases" }
    var failed = 0
    for (i in 0 until cases.length()) {
        val case = cases.getJSONObject(i)
        val pending = case.getJSONArray("pending")
        val reviews =
            (0 until pending.length()).map {
                val r = pending.getJSONObject(it)
                // An observed answer (AnkiQuest Lite) has no time_ms.
                val timeMs = if (r.has("time_ms")) r.getLong("time_ms") else null
                PendingReview(id = r.getLong("id"), timeMs = timeMs)
            }
        val actual = shape(todayVerdict(server(case.getJSONObject("server")), reviews, case.getLong("now_ms")))
        val expected = case.get("expected")
        val same =
            if (expected is JSONObject && actual is JSONObject) {
                expected.similar(actual)
            } else {
                expected == actual
            }
        if (!same) {
            failed++
            println("FAIL ${case.getString("name")}: expected $expected, got $actual")
        }
    }
    println("${cases.length() - failed} of ${cases.length()} cases match")
    if (failed > 0) {
        exitProcess(1)
    }
}
