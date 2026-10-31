import { describe, expect, it } from "vitest";
import { renderStatusMessageSlot, STATUS_MESSAGE_HIDE_MS, StatusMessageSlot, type StatusMessageTimers } from "./status-message";

/** A clock the test advances by hand. */
function manualTimers(): StatusMessageTimers & { advance(ms: number): void; pending(): number } {
  let now = 0;
  let nextHandle = 1;
  const scheduled = new Map<number, Readonly<{ at: number; callback: () => void }>>();
  return {
    setTimeout: (callback, ms) => {
      const handle = nextHandle++;
      scheduled.set(handle, { at: now + ms, callback });
      return handle;
    },
    clearTimeout: (handle) => { scheduled.delete(handle as number); },
    advance: (ms) => {
      now += ms;
      for (const [handle, timer] of [...scheduled]) {
        if (timer.at > now) continue;
        scheduled.delete(handle);
        timer.callback();
      }
    },
    pending: () => scheduled.size,
  };
}

describe("status message slot", () => {
  it("hides a success after five seconds and says so", () => {
    const timers = manualTimers();
    let expired = 0;
    const slot = new StatusMessageSlot(() => { expired += 1; }, timers);
    slot.set({ message: "Reloaded.", failed: false });

    timers.advance(STATUS_MESSAGE_HIDE_MS - 1);
    expect(slot.current()?.message).toBe("Reloaded.");
    timers.advance(1);
    expect(slot.current()).toBeNull();
    expect(expired).toBe(1);
  });

  it("keeps an error until it is cleared or replaced", () => {
    const timers = manualTimers();
    const slot = new StatusMessageSlot(() => undefined, timers);
    slot.set({ message: "Refused.", failed: true });
    expect(timers.pending()).toBe(0);
    timers.advance(STATUS_MESSAGE_HIDE_MS * 10);
    expect(slot.current()?.message).toBe("Refused.");

    slot.set({ message: "Reloaded.", failed: false });
    expect(slot.current()?.message).toBe("Reloaded.");
    slot.set({ message: "Refused again.", failed: true });
    // The earlier success's timer no longer applies.
    expect(timers.pending()).toBe(0);
    slot.clear();
    expect(slot.current()).toBeNull();
  });

  it("restarts the five seconds for each new success", () => {
    const timers = manualTimers();
    const slot = new StatusMessageSlot(() => undefined, timers);
    slot.set({ message: "First.", failed: false });
    timers.advance(STATUS_MESSAGE_HIDE_MS - 1);
    slot.set({ message: "Second.", failed: false });
    timers.advance(STATUS_MESSAGE_HIDE_MS - 1);
    expect(slot.current()?.message).toBe("Second.");
    timers.advance(1);
    expect(slot.current()).toBeNull();
  });

  it("renders a reserved, empty live region, a polite success, and an assertive error with Dismiss", () => {
    expect(renderStatusMessageSlot(null, "data-dismiss"))
      .toBe('<div class="status-message-slot"><div class="status-message" role="status" aria-live="polite"></div></div>');
    const success = renderStatusMessageSlot({ message: "Saved <now>.", failed: false }, "data-dismiss");
    expect(success).toContain('role="status" aria-live="polite"><p>Saved &lt;now&gt;.</p>');
    expect(success).not.toContain("data-dismiss");
    const error = renderStatusMessageSlot({ message: "Refused.", failed: true }, "data-dismiss");
    expect(error).toContain('role="alert" aria-live="assertive"><p>Refused.</p>');
    expect(error).toContain('data-dismiss aria-label="Dismiss message"');
  });
});
