import { describe, expect, it } from "vitest"

import {
  deriveBadge,
  formatDuration,
  parseStatusReport,
  parseTaskId,
} from "./delegation-status"

// Mirrors the MCP CallToolResult envelope the companion emits.
function envelope(report: Record<string, unknown>, isError = false): string {
  const text =
    report.status === "completed"
      ? ((report.text ?? report.message ?? "") as string)
      : ((report.message ?? report.text ?? "") as string)
  return JSON.stringify({
    content: [{ type: "text", text }],
    isError,
    structuredContent: report,
  })
}

describe("parseTaskId", () => {
  it("reads a plain task_id", () => {
    expect(parseTaskId(JSON.stringify({ task_id: "abc12345" }))).toBe(
      "abc12345"
    )
  })

  it("peels a double-encoded input", () => {
    const input = JSON.stringify(JSON.stringify({ task_id: "abc12345" }))
    expect(parseTaskId(input)).toBe("abc12345")
  })

  it("peels a host wrapper key (arguments as JSON string)", () => {
    const input = JSON.stringify({
      arguments: JSON.stringify({ task_id: "abc12345" }),
    })
    expect(parseTaskId(input)).toBe("abc12345")
  })

  it("returns null for unparseable / missing input", () => {
    expect(parseTaskId("not json")).toBeNull()
    expect(parseTaskId(null)).toBeNull()
    expect(parseTaskId(undefined)).toBeNull()
  })
})

describe("parseStatusReport", () => {
  it("recovers a structuredContent report (status + duration)", () => {
    const report = parseStatusReport(
      envelope({
        task_id: "abc12345",
        status: "completed",
        text: "All done.",
        duration_ms: 1234,
      }),
      null
    )
    expect(report.status).toBe("completed")
    expect(report.durationMs).toBe(1234)
    expect(report.text).toBe("All done.")
  })

  it("recovers a Codex-wrapped report (Wall time prefix)", () => {
    const inner = JSON.stringify({ task_id: "abc12345", status: "unknown" })
    const report = parseStatusReport(
      `Wall time: 1 seconds\nOutput:\n${inner}_`,
      null
    )
    expect(report.status).toBe("unknown")
  })

  it("does NOT treat a child's own JSON-with-status as a report (no task_id)", () => {
    const childOutput = JSON.stringify({ status: "failed", message: "child" })
    const report = parseStatusReport(childOutput, null)
    expect(report.status).toBeNull()
    expect(report.text).toBe(childOutput)
  })

  it("keeps plain text as the displayable result", () => {
    const report = parseStatusReport("Sub-agent finished.", null)
    expect(report.status).toBeNull()
    expect(report.text).toBe("Sub-agent finished.")
  })

  it("reads the running sentinel from content-only text (no structuredContent)", () => {
    // Historical Claude reload drops structuredContent; only the backend's
    // running message survives. It must still resolve to a running status so the
    // badge becomes the neutral 'checked', not a false 'ok' ("done").
    const report = parseStatusReport(
      "Sub-agent is still running in the background.",
      null
    )
    expect(report.status).toBe("running")
    expect(deriveBadge("status", report, "output-available", false)).toEqual({
      status: "checked",
    })
  })

  it("reads the bare 'Running.' running message from content-only text", () => {
    // Current backend baseline: running_report emits the bare "Running." On a
    // content-only host (structuredContent dropped) this is the only signal,
    // so it must resolve to running, not a false 'ok'.
    const report = parseStatusReport("Running.", null)
    expect(report.status).toBe("running")
    expect(deriveBadge("status", report, "output-available", false)).toEqual({
      status: "checked",
    })
  })

  it("reads the two-line 'Running.\\nLatest sub-agent reply: …' upgraded message", () => {
    // The live hint lands on its own line; the standalone first-line marker
    // identifies "still running" and the child's reply text is preserved.
    const report = parseStatusReport(
      "Running.\nLatest sub-agent reply: Reading config.rs",
      null
    )
    expect(report.status).toBe("running")
    expect(report.text).toBe(
      "Running.\nLatest sub-agent reply: Reading config.rs"
    )
  })

  it("does NOT misread a completed single-line 'Running. Latest sub-agent reply: …' result as running", () => {
    // A completed child answer whose text merely STARTS WITH the running phrase
    // on one line (no standalone marker line) must resolve to a terminal status,
    // not running — the reply text is child-controlled and never anchored on.
    const report = parseStatusReport(
      "Running. Latest sub-agent reply: I finished, nothing else is running.",
      null
    )
    expect(report.status).toBeNull()
  })

  it("does NOT treat a completed result whose first line only starts with 'Running' as running", () => {
    // Anchored on the WHOLE first line being exactly "Running.", so "Running…"
    // variants or "Running. <more on the same line>" do not match.
    expect(
      parseStatusReport("Running the migration now.", null).status
    ).toBeNull()
    expect(
      parseStatusReport(
        "Running.\nAll tests pass and nothing is pending.",
        null
      ).status
    ).toBeNull()
  })

  it("does NOT treat an ordinary completion result as still-running", () => {
    const report = parseStatusReport("The migration finished cleanly.", null)
    expect(report.status).toBeNull()
  })

  it("does NOT classify a result that merely mentions 'Running.' mid-text as running", () => {
    // Anchored match, not a loose substring: a completed child can incidentally
    // write the word.
    const report = parseStatusReport(
      "All checks passed. Running. tests are green now.",
      null
    )
    expect(report.status).toBeNull()
  })

  it("does NOT classify a longer result that merely quotes the sentinel as running", () => {
    // A completed child result could embed the phrase; only an exact-match
    // sentinel is the backend's running signal.
    const report = parseStatusReport(
      "I saw: Sub-agent is still running in the background. So I waited and it finished.",
      null
    )
    expect(report.status).toBeNull()
  })

  it("surfaces the report's task_id, and null when absent", () => {
    expect(
      parseStatusReport(
        envelope({ task_id: "abc12345", status: "completed", text: "ok" }),
        null
      ).taskId
    ).toBe("abc12345")
    expect(parseStatusReport("plain output", null).taskId).toBeNull()
  })
})

describe("deriveBadge", () => {
  const empty = parseStatusReport(null, null)

  it("maps a RETURNED running poll to the neutral checked state (not a spinner)", () => {
    const report = parseStatusReport(
      envelope({ task_id: "abc12345", status: "running", message: "working" }),
      null
    )
    expect(deriveBadge("status", report, "output-available", false)).toEqual({
      status: "checked",
    })
  })

  it("keeps a spinner for an in-flight poll (no result yet)", () => {
    expect(deriveBadge("status", empty, "input-available", false)).toEqual({
      status: "running",
    })
    expect(deriveBadge("status", empty, "input-streaming", false)).toEqual({
      status: "running",
    })
  })

  it("maps completed → ok", () => {
    const report = parseStatusReport(
      envelope({ task_id: "abc12345", status: "completed", text: "ok" }),
      null
    )
    expect(deriveBadge("status", report, "output-available", false)).toEqual({
      status: "ok",
    })
  })

  it("maps failed/unknown to errors", () => {
    const failed = parseStatusReport(
      envelope({ task_id: "x", status: "failed", error_code: "timeout" }, true),
      null
    )
    expect(deriveBadge("status", failed, "output-error", true)).toEqual({
      status: "err",
      errorCode: "timeout",
    })
    const unknown = parseStatusReport(
      envelope({ task_id: "x", status: "unknown" }),
      null
    )
    expect(deriveBadge("status", unknown, "output-available", false)).toEqual({
      status: "err",
      errorCode: "unknown",
    })
  })

  it("treats canceled as success for cancel, terminal error for a status query", () => {
    const report = parseStatusReport(
      envelope({ task_id: "x", status: "canceled", error_code: "canceled" }),
      null
    )
    expect(deriveBadge("cancel", report, "output-available", false)).toEqual({
      status: "ok",
    })
    expect(deriveBadge("status", report, "output-available", false)).toEqual({
      status: "err",
      errorCode: "canceled",
    })
  })

  it("falls back to ok for a returned poll with no parseable status", () => {
    const report = parseStatusReport("done", null)
    expect(deriveBadge("status", report, "output-available", false)).toEqual({
      status: "ok",
    })
  })
})

describe("formatDuration", () => {
  it("formats sub-second, second and minute spans", () => {
    expect(formatDuration(350)).toBe("350ms")
    expect(formatDuration(1234)).toBe("1.2s")
    expect(formatDuration(12_000)).toBe("12s")
    expect(formatDuration(119_999)).toBe("2m 0s")
  })
})
