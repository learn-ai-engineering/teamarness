import { describe, expect, it } from "vitest";
import {
  TEAMARNESS_THEME_TOKENS,
  type PackageMountContext,
  type PackageTransport,
} from "@teamarness/package-ui-sdk";
import type { OpenPackageMount, PackageInstanceKey } from "./package-contracts";
import { PackageMountController, type PackageMountBridge, type PackageMountDom } from "./package-mount";

const instance: PackageInstanceKey = {
  packageIdentity: "com.example.fixture",
  workspaceId: "00112233445566778899aabbccddeeff",
};

function transport(): PackageTransport {
  return {
    request: async () => null,
    cancelRequest: async () => undefined,
    openSubscription: async () => undefined,
    acknowledgeEvents: async () => undefined,
    closeSubscription: async () => undefined,
    reportUiFault: async () => undefined,
  };
}

function descriptor(number: number): OpenPackageMount {
  return {
    mount: `mount-${number}`,
    loader: {
      token: `${String(number).repeat(32)}`,
      entrypoint: "ui/index.js",
      uiHostVersion: 1,
      contractDigest: "a".repeat(64),
      package: {
        identity: instance.packageIdentity,
        version: "1.0.0",
        displayName: "Fixture",
        source: "bundled",
        trust: "application_bundled",
        lifecycle: "active",
        processGeneration: 1,
      },
      workspace: { id: instance.workspaceId, name: "Fixture Workspace" },
    },
  };
}

function harness(): {
  bridge: PackageMountBridge;
  dom: PackageMountDom;
  calls: string[];
  contexts: PackageMountContext[];
  tokens: string[][];
} {
  const calls: string[] = [];
  const contexts: PackageMountContext[] = [];
  const tokens: string[][] = [];
  let opened = 0;
  const bridge: PackageMountBridge = {
    transport: transport(),
    openMount: async () => descriptor(++opened),
    closeMount: async ({ mount }) => { calls.push(`close:${mount}`); },
    reportUiFault: async ({ mount }) => { calls.push(`fault:${mount}`); },
    suspendBackend: async (selected) => { calls.push(`suspend:${selected.packageIdentity}`); },
    recoverBackend: async (selected) => { calls.push(`recover:${selected.packageIdentity}`); },
    disposeBackend: async (selected) => { calls.push(`dispose:${selected.packageIdentity}`); },
  };
  const dom: PackageMountDom = {
    create: () => ({
      root: {} as ShadowRoot,
      setThemeTokens: (names) => { tokens.push([...names]); },
      remove: () => { calls.push("remove"); },
    }),
    watchErrors: () => () => { calls.push("unwatch"); },
  };
  return { bridge, dom, calls, contexts, tokens };
}

describe("generic Package UI mount", () => {
  it("uses a fresh Shadow Root with inherited token names and generic context", async () => {
    const test = harness();
    let loadedUrl = "";
    const controller = new PackageMountController(test.bridge, test.dom, async (url) => {
      loadedUrl = url;
      return { mount: async (context) => { test.contexts.push(context); return async () => undefined; } };
    });
    await controller.mount(instance, {});
    expect(loadedUrl).toBe(`teamarness-package://localhost/${"1".repeat(32)}/ui/index.js`);
    expect(test.tokens).toEqual([[...TEAMARNESS_THEME_TOKENS]]);
    expect(test.contexts[0]?.themeTokens).toBe(TEAMARNESS_THEME_TOKENS);
    expect(test.contexts[0]).toMatchObject({
      package: {
        identity: instance.packageIdentity,
        version: "1.0.0",
        displayName: "Fixture",
        source: "bundled",
        trust: "application_bundled",
        lifecycle: "active",
        processGeneration: 1,
      },
      workspace: { id: instance.workspaceId, name: "Fixture Workspace" },
      uiHostVersion: 1,
    });
    expect(JSON.stringify(test.contexts[0])).not.toContain("1".repeat(32));
  });

  it("routes generic lifecycle actions through the bridge for the supplied instance", async () => {
    const test = harness();
    const controller = new PackageMountController(test.bridge, test.dom, async () => ({
      mount: async () => async () => undefined,
    }));

    await controller.suspendBackend(instance);
    await controller.recoverBackend(instance);
    await controller.disposeBackend(instance);

    expect(test.calls).toEqual([
      "suspend:com.example.fixture",
      "recover:com.example.fixture",
      "dispose:com.example.fixture",
    ]);
  });

  it("unmounts idempotently and replaces a mount without recovering its Backend", async () => {
    const test = harness();
    const controller = new PackageMountController(test.bridge, test.dom, async () => ({
      mount: async () => async () => { test.calls.push("package-unmount"); },
    }));
    const first = await controller.mount(instance, {});
    await controller.mount(instance, {});
    await first.unmount();
    await controller.unmount();
    await controller.unmount();
    expect(test.calls).toEqual([
      "package-unmount", "unwatch", "close:mount-1", "remove",
      "package-unmount", "unwatch", "close:mount-2", "remove",
    ]);
    expect(test.calls).not.toContain("recover");
  });

  it("surfaces a terminal Backend stream failure to the mount owner", async () => {
    const test = harness();
    let terminate: (() => void) | undefined;
    test.bridge.transport.openSubscription = async (_input, _deliver, onFailure) => {
      terminate = onFailure;
    };
    const controller = new PackageMountController(test.bridge, test.dom, async () => ({
      mount: async ({ client }) => {
        await client.subscribe("fixture.changes", {}, async () => undefined);
        return async () => undefined;
      },
    }));
    let failures = 0;
    await controller.mount(instance, {}, () => { failures += 1; });

    terminate?.();
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(failures).toBe(1);
    expect(test.calls).toContain("fault:mount-1");
    expect(test.calls).not.toContain("recover");
  });

  it("runs every local cleanup stage after ordinary unmount failures and retries only incomplete host close", async () => {
    const test = harness();
    const packageError = new Error("package unmount failed");
    let closeAttempts = 0;
    test.bridge.reportUiFault = async ({ mount }) => {
      test.calls.push(`fault:${mount}`);
      throw new Error("fault report failed");
    };
    test.bridge.transport.closeSubscription = async () => {
      test.calls.push("dispose-client");
      throw new Error("client dispose failed");
    };
    test.bridge.closeMount = async ({ mount }) => {
      closeAttempts += 1;
      test.calls.push(`close:${mount}:${String(closeAttempts)}`);
      if (closeAttempts === 1) throw new Error("host close failed");
    };
    const controller = new PackageMountController(test.bridge, test.dom, async () => ({
      mount: async ({ client }) => {
        await client.subscribe("fixture.changes", {}, async () => undefined);
        return async () => {
          test.calls.push("package-unmount");
          throw packageError;
        };
      },
    }));
    await controller.mount(instance, {});

    await expect(controller.unmount()).rejects.toBe(packageError);
    expect(test.calls).toEqual([
      "package-unmount",
      "fault:mount-1",
      "unwatch",
      "dispose-client",
      "close:mount-1:1",
      "remove",
    ]);

    await expect(controller.unmount()).resolves.toBeUndefined();
    expect(test.calls).toEqual([
      "package-unmount",
      "fault:mount-1",
      "unwatch",
      "dispose-client",
      "close:mount-1:1",
      "remove",
      "close:mount-1:2",
    ]);
  });

  it("preserves the mount failure while cleanup continues and incomplete host close remains retryable", async () => {
    const test = harness();
    const importError = new Error("import failed");
    let closeAttempts = 0;
    test.bridge.reportUiFault = async ({ mount }) => {
      test.calls.push(`fault:${mount}`);
      throw new Error("fault report failed");
    };
    test.bridge.closeMount = async ({ mount }) => {
      closeAttempts += 1;
      test.calls.push(`close:${mount}:${String(closeAttempts)}`);
      if (closeAttempts === 1) throw new Error("host close failed");
    };
    const controller = new PackageMountController(test.bridge, test.dom, async () => { throw importError; });

    await expect(controller.mount(instance, {})).rejects.toBe(importError);
    expect(test.calls).toEqual([
      "fault:mount-1",
      "unwatch",
      "close:mount-1:1",
      "remove",
    ]);

    await expect(controller.unmount()).resolves.toBeUndefined();
    expect(test.calls).toEqual([
      "fault:mount-1",
      "unwatch",
      "close:mount-1:1",
      "remove",
      "close:mount-1:2",
    ]);
  });

  it("reports import and attributed UI failures without recovering the Backend", async () => {
    const test = harness();
    let attributedFault: (() => void) | undefined;
    test.dom.watchErrors = (_url, fault) => { attributedFault = fault; return () => undefined; };
    const controller = new PackageMountController(test.bridge, test.dom, async () => ({
      mount: async () => async () => undefined,
    }));
    await controller.mount(instance, {});
    attributedFault?.();
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(test.calls).toEqual(["fault:mount-1"]);

    const broken = new PackageMountController(test.bridge, test.dom, async () => { throw new Error("import"); });
    await expect(broken.mount(instance, {})).rejects.toThrow("import");
    expect(test.calls).toContain("fault:mount-2");
    expect(test.calls).not.toContain("recover");
  });
});
