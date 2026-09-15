import { describe, expect, it } from "vitest";
import { fmtBytes, fmtMs, identityIdx, isTaskActive, relTime, taskStateDisplay, taskTone } from "./format";

describe("fmtMs — mirrors Rust fmt_ms", () => {
  it("formats each bracket", () => {
    expect(fmtMs(812)).toBe("812 ms");
    expect(fmtMs(5400)).toBe("5.4 s");
    expect(fmtMs(38_000)).toBe("38 s");
    expect(fmtMs(185_000)).toBe("3m 05s");
    expect(fmtMs(3_720_000)).toBe("1h 02m");
  });
});

describe("fmtBytes — mirrors Rust fmt_bytes", () => {
  it("formats each bracket", () => {
    expect(fmtBytes(271)).toBe("271 B");
    expect(fmtBytes(1234)).toBe("1.2 kB");
    expect(fmtBytes(Math.round(3.4 * 1024 * 1024))).toBe("3.4 MB");
    expect(fmtBytes(Math.round(1.1 * 1024 ** 3))).toBe("1.1 GB");
  });
});

describe("taskStateDisplay", () => {
  it("strips the TASK_STATE_ prefix and canonicalizes dashes", () => {
    expect(taskStateDisplay("TASK_STATE_INPUT_REQUIRED")).toBe("input-required");
    expect(taskStateDisplay("TASK_STATE_COMPLETED")).toBe("completed");
    expect(taskStateDisplay("task_state_working")).toBe("working");
    expect(taskStateDisplay(null)).toBe("—");
  });
});

describe("taskTone / isTaskActive", () => {
  it("classifies lifecycle buckets", () => {
    expect(taskTone("TASK_STATE_FAILED")).toBe("bad");
    expect(taskTone("TASK_STATE_INPUT_REQUIRED")).toBe("warn");
    expect(taskTone("TASK_STATE_COMPLETED")).toBe("ok");
    expect(taskTone("TASK_STATE_WORKING")).toBe("");
    expect(isTaskActive("TASK_STATE_WORKING")).toBe(true);
    expect(isTaskActive("TASK_STATE_COMPLETED")).toBe(false);
    expect(isTaskActive("TASK_STATE_INPUT_REQUIRED")).toBe(true);
  });
});

describe("relTime", () => {
  it("renders relative labels", () => {
    const now = 1_000_000;
    expect(relTime(now - 10, now)).toBe("just now");
    expect(relTime(now - 600, now)).toBe("10 min ago");
    expect(relTime(now - 7200, now)).toBe("2 h ago");
    expect(relTime(now - 3 * 86_400, now)).toBe("3 d ago");
    expect(relTime(null, now)).toBe("—");
  });
});

describe("identityIdx", () => {
  it("is stable and bounded", () => {
    for (const n of ["HB", "research-agent", "ops-agent", "gateway", "🙂-agent"]) {
      const i = identityIdx(n);
      expect(i).toBe(identityIdx(n));
      expect(i).toBeGreaterThanOrEqual(0);
      expect(i).toBeLessThan(8);
    }
    expect(identityIdx("a")).toBe(identityIdx("a"));
  });
});
