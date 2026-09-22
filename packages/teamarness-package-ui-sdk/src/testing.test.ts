import { describe, expect, it } from "vitest";

import { PACKAGE_REQUEST_ERROR_KIND, isPackageRequestError, packageErrorCode } from "./index";
import type { PackageRequestError, PackageValue } from "./index";
import {
  createFakePackageClient,
  createPackageRequestError,
  createTestMountContext,
} from "./testing";

describe("createPackageRequestError", () => {
  it("createPackageRequestError builds a branded error recognised by isPackageRequestError", () => {
    const plain = createPackageRequestError();
    expect(plain.kind).toBe(PACKAGE_REQUEST_ERROR_KIND);
    expect(isPackageRequestError(plain)).toBe(true);
    expect(Object.isFrozen(plain)).toBe(true);
    expect(plain.packageError).toBeNull();

    const owned = createPackageRequestError({
      code: "backend_error",
      stage: "backend",
      packageError: { code: "demo.rejected", retryable: true, details: { reason: "no" } },
    });
    expect(isPackageRequestError(owned)).toBe(true);
    expect(packageErrorCode(owned)).toBe("demo.rejected");
    expect(owned.packageError?.retryable).toBe(true);
    expect(owned.packageError?.details.reason).toBe("no");
  });
});

describe("createFakePackageClient", () => {
  it("fake client records calls, answers handlers, and rejects unknown operations as ui_contract_violation", async () => {
    const fake = createFakePackageClient({
      requests: {
        "demo.read": (payload) => ({ echoed: payload }),
      },
    });

    await expect(fake.client.request("demo.read", { a: 1 })).resolves.toEqual({
      echoed: { a: 1 },
    });
    expect(fake.calls).toEqual([{ operation: "demo.read", payload: { a: 1 } }]);

    const failure = await fake.client
      .request("demo.absent", {})
      .then(() => null, (error: unknown) => error);
    expect(isPackageRequestError(failure)).toBe(true);
    expect((failure as PackageRequestError).code).toBe("ui_contract_violation");
    expect((failure as PackageRequestError).stage).toBe("ui");
    expect(fake.calls).toHaveLength(2);
  });

  it("fake subscriptions deliver emitted events and terminate with an error or null", async () => {
    const fake = createFakePackageClient({ subscriptions: { "demo.changes": () => undefined } });
    const events: PackageValue[] = [];
    const endings: Array<PackageRequestError | null> = [];

    const subscription = await fake.client.subscribe(
      "demo.changes",
      {},
      (event) => {
        events.push(event);
      },
      { onTerminated: (error) => endings.push(error) },
    );
    await fake.emit("demo.changes", { step: 1 });
    await fake.emit("demo.changes", { step: 2 });
    expect(events).toEqual([{ step: 1 }, { step: 2 }]);

    fake.terminate("demo.changes", createPackageRequestError({ code: "overloaded" }));
    expect(endings).toHaveLength(1);
    expect(endings[0]?.code).toBe("overloaded");

    // A terminated subscription receives nothing further, and closing it is harmless.
    await fake.emit("demo.changes", { step: 3 });
    expect(events).toHaveLength(2);
    await subscription.close();

    const second = createFakePackageClient({ subscriptions: { "demo.changes": () => undefined } });
    const normal: Array<PackageRequestError | null> = [];
    await second.client.subscribe("demo.changes", {}, () => undefined, {
      onTerminated: (error) => normal.push(error),
    });
    second.terminate("demo.changes", null);
    expect(normal).toEqual([null]);
  });

  it("rejects a subscription the handlers do not describe", async () => {
    const fake = createFakePackageClient();
    const failure = await fake.client
      .subscribe("demo.absent", {}, () => undefined)
      .then(() => null, (error: unknown) => error);
    expect(packageErrorCode(failure)).toBeNull();
    expect((failure as PackageRequestError).code).toBe("ui_contract_violation");
  });
});

describe("createTestMountContext", () => {
  it("builds the mount context a Package UI is given", () => {
    const fake = createFakePackageClient();
    const root = {} as ShadowRoot;
    const context = createTestMountContext({ root, client: fake.client, displayName: "Demo" });
    expect(context.root).toBe(root);
    expect(context.client).toBe(fake.client);
    expect(context.package.displayName).toBe("Demo");
    expect(context.package.processGeneration).toBe("1");
    expect(context.uiHostVersion).toBe(1);
    expect(context.themeTokens).toContain("--teamarness-color-accent");
  });
});
