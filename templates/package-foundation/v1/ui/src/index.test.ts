import type { PackageClient, PackageMountContext, PackageValue } from "@teamarness/package-ui-sdk";
import { describe, expect, it } from "vitest";

import { mount } from "./index";

class FakeElement {
  readonly children: FakeElement[] = [];
  parent: FakeElement | undefined;
  textContent = "";

  constructor(readonly tagName: string) {}

  append(...children: FakeElement[]): void {
    for (const child of children) {
      child.parent = this;
      this.children.push(child);
    }
  }

  remove(): void {
    if (!this.parent) return;
    const index = this.parent.children.indexOf(this);
    if (index >= 0) this.parent.children.splice(index, 1);
    this.parent = undefined;
  }
}

class FakeDocument {
  createElement(tagName: string): FakeElement {
    return new FakeElement(tagName);
  }
}

globalThis.document = new FakeDocument() as unknown as Document;

describe("generated Package UI", () => {
  it("mounts through the scoped PackageClient and cleans up idempotently", async () => {
    const root = new FakeElement("shadow-root");
    const requests: Array<{ operation: string; payload: PackageValue }> = [];
    const client: PackageClient = {
      request: async <TResponse extends PackageValue>(operation: string, payload: PackageValue): Promise<TResponse> => {
        requests.push({ operation, payload });
        return { message: "Hello from __REQUEST_NAMESPACE__" } as unknown as TResponse;
      },
      subscribe: async () => ({ close: async () => undefined }),
    };
    const context = {
      root,
      package: { displayName: __PACKAGE_DISPLAY_NAME_JSON__ },
      workspace: { id: "workspace", name: "Workspace" },
      uiHostVersion: 1,
      themeTokens: [],
      client,
    } as unknown as PackageMountContext;

    const unmount = await mount(context);

    expect(requests).toEqual([{ operation: "__REQUEST_NAMESPACE__.read", payload: {} }]);
    expect(root.children).toHaveLength(1);
    expect(root.children[0]?.children.map(({ textContent }) => textContent)).toEqual([
      __PACKAGE_DISPLAY_NAME_JSON__,
      "Hello from __REQUEST_NAMESPACE__",
    ]);

    unmount();
    unmount();
    expect(root.children).toHaveLength(0);
  });
});
