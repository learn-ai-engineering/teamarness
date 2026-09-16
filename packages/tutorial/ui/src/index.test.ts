import type { PackageClient, PackageValue } from "@teamarness/package-ui-sdk";
import { describe, expect, it } from "vitest";

import { createTutorialController, type Draft, type TutorialState, type Recipe, type SavedArtwork } from "./index";

const recipe = (seed: number, algorithm = "waves", palette = "garden"): Recipe => ({
  generatorVersion: 1,
  algorithm,
  palette,
  seed,
  width: 16,
  height: 12,
});
const draft = (seed: number, algorithm = "waves", palette = "garden"): Draft => ({
  recipe: recipe(seed, algorithm, palette),
  cells: Array.from({ length: 192 }, () => "🌱"),
});

class FakeClient implements PackageClient {
  readonly requests: Array<{ operation: string; payload: PackageValue }> = [];
  readonly generatedSeeds = [11, 12, 13];
  failNext = false;

  async request<TResponse extends PackageValue>(operation: string, payload: PackageValue): Promise<TResponse> {
    this.requests.push({ operation, payload });
    if (this.failNext) {
      this.failNext = false;
      throw new Error("Backend unavailable");
    }
    if (operation === "tutorial.read") {
      return {
        algorithms: [{ id: "waves", label: "Waves" }, { id: "ripples", label: "Ripples" }, { id: "spiral", label: "Spiral" }, { id: "patchwork", label: "Patchwork" }],
        palettes: [
          { id: "garden", label: "Garden", values: ["🌱", "🌿", "🍀", "🌸", "🌻"] },
          { id: "ocean", label: "Ocean", values: ["💧", "🐚", "🐠", "🐬", "🌊"] },
          { id: "moon", label: "Moon", values: ["🌑", "🌒", "🌓", "🌔", "🌕"] },
          { id: "fruit", label: "Fruit", values: ["🍓", "🍊", "🍋", "🍏", "🫐"] },
        ],
        artworks: [],
      } as unknown as TResponse;
    }
    if (operation === "tutorial.generate") {
      const selection = payload as { algorithm: string; palette: string };
      return draft(this.generatedSeeds.shift()!, selection.algorithm, selection.palette) as unknown as TResponse;
    }
    if (operation === "tutorial.save") {
      return { id: "artwork-1", ...draft(11) } as unknown as TResponse;
    }
    throw new Error(`Unexpected operation ${operation}`);
  }

  async subscribe(): Promise<never> {
    throw new Error("Tutorial has no subscriptions");
  }
}

describe("Tutorial UI client flow", () => {
  it("reads choices, selects their first values, and generates the initial draft", async () => {
    const client = new FakeClient();
    let state: TutorialState | undefined;
    const controller = createTutorialController(client, (next) => { state = next; });

    await controller.start();

    expect(client.requests).toEqual([
      { operation: "tutorial.read", payload: {} },
      { operation: "tutorial.generate", payload: { algorithm: "waves", palette: "garden" } },
    ]);
    expect(state?.draft?.recipe.seed).toBe(11);
  });

  it("generates again without changing the selected algorithm or palette", async () => {
    const client = new FakeClient();
    const controller = createTutorialController(client, () => {});
    await controller.start();
    controller.setAlgorithm("ripples");
    controller.setPalette("ocean");

    await controller.generate();
    await controller.generate();

    expect(client.requests.slice(-2)).toEqual([
      { operation: "tutorial.generate", payload: { algorithm: "ripples", palette: "ocean" } },
      { operation: "tutorial.generate", payload: { algorithm: "ripples", palette: "ocean" } },
    ]);
  });

  it("supports every Backend-owned pattern and palette choice", async () => {
    const client = new FakeClient();
    const controller = createTutorialController(client, () => {});
    await controller.start();
    const choices: Array<readonly [string, string]> = [["waves", "garden"], ["ripples", "ocean"], ["spiral", "moon"], ["patchwork", "fruit"]];
    for (const [algorithm, palette] of choices) {
      controller.setAlgorithm(algorithm);
      controller.setPalette(palette);
      await controller.generate();
    }
    expect(client.requests.slice(-4).map(({ payload }) => payload)).toEqual([
      { algorithm: "waves", palette: "garden" },
      { algorithm: "ripples", palette: "ocean" },
      { algorithm: "spiral", palette: "moon" },
      { algorithm: "patchwork", palette: "fruit" },
    ]);
  });

  it("saves the visible recipe and restores a saved piece without generating", async () => {
    const client = new FakeClient();
    let state: TutorialState | undefined;
    const controller = createTutorialController(client, (next) => { state = next; });
    await controller.start();

    await controller.save();
    const saved = state?.artworks[0] as SavedArtwork;
    const requestCount = client.requests.length;
    controller.selectArtwork(saved.id);

    expect(client.requests.at(-1)).toEqual({ operation: "tutorial.save", payload: { recipe: recipe(11) } });
    expect(client.requests).toHaveLength(requestCount);
    expect(state?.draft).toEqual({ recipe: saved.recipe, cells: saved.cells });
    expect(state?.selectedArtworkId).toBe("artwork-1");
  });

  it("presents request errors and cleanup is idempotent", async () => {
    const client = new FakeClient();
    const states: TutorialState[] = [];
    const controller = createTutorialController(client, (state) => { states.push(state); });
    await controller.start();
    client.failNext = true;

    await controller.generate();
    controller.dispose();
    controller.dispose();
    const stateCount = states.length;
    await controller.generate();

    expect(states.at(-1)?.error).toBe("Backend unavailable");
    expect(states).toHaveLength(stateCount);
  });
});
