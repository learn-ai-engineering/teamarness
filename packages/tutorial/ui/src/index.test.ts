import { createFakePackageClient, type FakePackageClient } from "@teamarness/package-ui-sdk/testing";
import { describe, expect, it } from "vitest";

import {
  createTutorialController,
  type AlgorithmId,
  type Draft,
  type PaletteId,
  type Recipe,
  type SavedArtwork,
  type TutorialState,
} from "./index";

const recipe = (seed: number, algorithm: AlgorithmId = "waves", palette: PaletteId = "garden"): Recipe => ({
  generatorVersion: 1,
  algorithm,
  palette,
  seed,
  width: 16,
  height: 12,
});
const draft = (seed: number, algorithm: AlgorithmId = "waves", palette: PaletteId = "garden"): Draft => ({
  recipe: recipe(seed, algorithm, palette),
  cells: Array.from({ length: 192 }, () => "🌱") as unknown as Draft["cells"],
});

type TutorialFake = FakePackageClient & { failNext(): void };

/** The Backend as the contract describes it, so the controller is tested against real shapes. */
function tutorialBackend(): TutorialFake {
  const seeds = [11, 12, 13];
  let failing = false;
  const guard = <T>(answer: () => T): T => {
    if (failing) {
      failing = false;
      throw new Error("Backend unavailable");
    }
    return answer();
  };
  const fake = createFakePackageClient({
    requests: {
      "tutorial.read": () => guard(() => ({
        algorithms: [
          { id: "waves", label: "Waves" },
          { id: "ripples", label: "Ripples" },
          { id: "spiral", label: "Spiral" },
          { id: "patchwork", label: "Patchwork" },
        ],
        palettes: [
          { id: "garden", label: "Garden", values: ["🌱", "🌿", "🍀", "🌸", "🌻"] },
          { id: "ocean", label: "Ocean", values: ["💧", "🐚", "🐠", "🐬", "🌊"] },
          { id: "moon", label: "Moon", values: ["🌑", "🌒", "🌓", "🌔", "🌕"] },
          { id: "fruit", label: "Fruit", values: ["🍓", "🍊", "🍋", "🍏", "🫐"] },
        ],
        artworks: [],
      })),
      "tutorial.generate": (payload) => guard(() => {
        const selection = payload as { algorithm: AlgorithmId; palette: PaletteId };
        return draft(seeds.shift()!, selection.algorithm, selection.palette);
      }),
      "tutorial.save": () => guard(() => ({ id: "artwork-1", ...draft(11) })),
    },
  });
  return Object.freeze({ ...fake, failNext: () => { failing = true; } });
}

describe("Tutorial UI client flow", () => {
  it("reads choices, selects their first values, and generates the initial draft", async () => {
    const client = tutorialBackend();
    let state: TutorialState | undefined;
    const controller = createTutorialController(client.client, (next) => { state = next; });

    await controller.start();

    expect(client.calls).toEqual([
      { operation: "tutorial.read", payload: {} },
      { operation: "tutorial.generate", payload: { algorithm: "waves", palette: "garden" } },
    ]);
    expect(state?.draft?.recipe.seed).toBe(11);
  });

  it("generates again without changing the selected algorithm or palette", async () => {
    const client = tutorialBackend();
    const controller = createTutorialController(client.client, () => {});
    await controller.start();
    controller.setAlgorithm("ripples");
    controller.setPalette("ocean");

    await controller.generate();
    await controller.generate();

    expect(client.calls.slice(-2)).toEqual([
      { operation: "tutorial.generate", payload: { algorithm: "ripples", palette: "ocean" } },
      { operation: "tutorial.generate", payload: { algorithm: "ripples", palette: "ocean" } },
    ]);
  });

  it("supports every Backend-owned pattern and palette choice", async () => {
    const client = tutorialBackend();
    const controller = createTutorialController(client.client, () => {});
    await controller.start();
    const choices: Array<readonly [string, string]> = [["waves", "garden"], ["ripples", "ocean"], ["spiral", "moon"], ["patchwork", "fruit"]];
    for (const [algorithm, palette] of choices) {
      controller.setAlgorithm(algorithm);
      controller.setPalette(palette);
      await controller.generate();
    }
    expect(client.calls.slice(-4).map(({ payload }) => payload)).toEqual([
      { algorithm: "waves", palette: "garden" },
      { algorithm: "ripples", palette: "ocean" },
      { algorithm: "spiral", palette: "moon" },
      { algorithm: "patchwork", palette: "fruit" },
    ]);
  });

  it("saves the visible recipe and restores a saved piece without generating", async () => {
    const client = tutorialBackend();
    let state: TutorialState | undefined;
    const controller = createTutorialController(client.client, (next) => { state = next; });
    await controller.start();

    await controller.save();
    const saved = state?.artworks[0] as SavedArtwork;
    const requestCount = client.calls.length;
    controller.selectArtwork(saved.id);

    expect(client.calls.at(-1)).toEqual({ operation: "tutorial.save", payload: { recipe: recipe(11) } });
    expect(client.calls).toHaveLength(requestCount);
    expect(state?.draft).toEqual({ recipe: saved.recipe, cells: saved.cells });
    expect(state?.selectedArtworkId).toBe("artwork-1");
  });

  it("presents request errors and cleanup is idempotent", async () => {
    const client = tutorialBackend();
    const states: TutorialState[] = [];
    const controller = createTutorialController(client.client, (state) => { states.push(state); });
    await controller.start();
    client.failNext();

    await controller.generate();
    controller.dispose();
    controller.dispose();
    const stateCount = states.length;
    await controller.generate();

    expect(states.at(-1)?.error).toBe("Backend unavailable");
    expect(states).toHaveLength(stateCount);
  });
});
