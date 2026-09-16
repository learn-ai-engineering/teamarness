export async function waitForDesktopCleanup(waitForCleanup, emergencyTerminate) {
  try {
    await waitForCleanup();
  } catch (error) {
    try {
      emergencyTerminate("SIGKILL");
    } catch {
      // Keep the formatted cleanup timeout as the failure reported by the smoke.
    }
    throw error;
  }
}
