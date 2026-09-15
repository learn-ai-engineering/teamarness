import type {
  PackageClient,
  PackageMountContext,
  PackageUnmount,
} from "@teamarness/package-ui-sdk";

export type Choice = Readonly<{ id: string; label: string }>;
export type PaletteChoice = Readonly<{ id: string; label: string; values: readonly string[] }>;
export type Recipe = Readonly<{
  generatorVersion: number;
  algorithm: string;
  palette: string;
  seed: number;
  width: number;
  height: number;
}>;
export type Draft = Readonly<{ recipe: Recipe; cells: readonly string[] }>;
export type SavedArtwork = Readonly<{ id: string; recipe: Recipe; cells: readonly string[] }>;
type ReadResponse = Readonly<{
  algorithms: readonly Choice[];
  palettes: readonly PaletteChoice[];
  artworks: readonly SavedArtwork[];
}>;

export type TutorialState = Readonly<{
  algorithms: readonly Choice[];
  palettes: readonly PaletteChoice[];
  artworks: readonly SavedArtwork[];
  algorithm: string;
  palette: string;
  draft?: Draft;
  selectedArtworkId?: string;
  error?: string;
}>;

export type TutorialController = Readonly<{
  start(): Promise<void>;
  setAlgorithm(value: string): void;
  setPalette(value: string): void;
  generate(): Promise<void>;
  save(): Promise<void>;
  selectArtwork(id: string): void;
  dispose(): void;
}>;

export function createTutorialController(
  client: PackageClient,
  present: (state: TutorialState) => void,
): TutorialController {
  let disposed = false;
  let state: TutorialState = {
    algorithms: [],
    palettes: [],
    artworks: [],
    algorithm: "",
    palette: "",
  };

  const update = (changes: Partial<TutorialState>): void => {
    if (disposed) return;
    state = { ...state, ...changes };
    present(state);
  };
  const fail = (error: unknown): void => {
    update({ error: error instanceof Error ? error.message : "Tutorial request failed" });
  };
  const generate = async (): Promise<void> => {
    if (disposed || !state.algorithm || !state.palette) return;
    try {
      const result = await client.request("tutorial.generate", {
        algorithm: state.algorithm,
        palette: state.palette,
      }) as Draft;
      update({ draft: result, selectedArtworkId: undefined, error: undefined });
    } catch (error) {
      fail(error);
    }
  };

  return Object.freeze({
    async start(): Promise<void> {
      if (disposed) return;
      try {
        const result = await client.request("tutorial.read", {}) as ReadResponse;
        const algorithm = result.algorithms[0]?.id;
        const palette = result.palettes[0]?.id;
        if (!algorithm || !palette) throw new Error("Tutorial returned no generation choices");
        update({
          algorithms: result.algorithms,
          palettes: result.palettes,
          artworks: result.artworks,
          algorithm,
          palette,
          error: undefined,
        });
        await generate();
      } catch (error) {
        fail(error);
      }
    },
    setAlgorithm(value: string): void {
      if (state.algorithms.some(({ id }) => id === value)) {
        update({ algorithm: value, selectedArtworkId: undefined });
      }
    },
    setPalette(value: string): void {
      if (state.palettes.some(({ id }) => id === value)) {
        update({ palette: value, selectedArtworkId: undefined });
      }
    },
    generate,
    async save(): Promise<void> {
      if (disposed || !state.draft) return;
      try {
        const saved = await client.request("tutorial.save", { recipe: state.draft.recipe }) as SavedArtwork;
        update({
          artworks: [...state.artworks, saved],
          draft: { recipe: saved.recipe, cells: saved.cells },
          selectedArtworkId: saved.id,
          error: undefined,
        });
      } catch (error) {
        fail(error);
      }
    },
    selectArtwork(id: string): void {
      const saved = state.artworks.find((artwork) => artwork.id === id);
      if (!saved) return;
      update({
        algorithm: saved.recipe.algorithm,
        palette: saved.recipe.palette,
        draft: { recipe: saved.recipe, cells: saved.cells },
        selectedArtworkId: saved.id,
        error: undefined,
      });
    },
    dispose(): void {
      disposed = true;
    },
  });
}

export async function mount(context: PackageMountContext): Promise<PackageUnmount> {
  const style = document.createElement("style");
  style.textContent = `
    :host { color: var(--teamarness-color-text); font-family: var(--teamarness-font-body); }
    article { display: grid; gap: .9rem; padding: 1rem; }
    h2, p { margin: 0; }
    .controls { display: flex; flex-wrap: wrap; gap: .7rem; align-items: end; }
    label { display: grid; gap: .25rem; font-weight: 650; }
    select, button { font: inherit; border-radius: var(--teamarness-radius-control); padding: .45rem .65rem; }
    button { border: 0; background: var(--teamarness-color-accent); cursor: pointer; }
    .art { display: grid; grid-template-columns: repeat(16, 1.75rem); grid-auto-rows: 1.75rem; width: fit-content; max-width: 100%; overflow: auto; }
    .cell { display: grid; place-items: center; font-size: 1.25rem; line-height: 1; }
    .error { color: var(--teamarness-color-danger, #b42318); min-height: 1.2em; }
    .saved { display: flex; flex-wrap: wrap; gap: .4rem; }
    .saved button[aria-pressed="true"] { outline: 2px solid currentColor; }
  `;
  const article = document.createElement("article");
  const heading = document.createElement("h2");
  heading.textContent = context.package.displayName;
  const controls = document.createElement("div");
  controls.className = "controls";
  const algorithm = selectControl("Pattern");
  const palette = selectControl("Palette");
  const generate = button("Generate");
  const save = button("Save");
  controls.append(algorithm.label, palette.label, generate, save);
  const error = document.createElement("p");
  error.className = "error";
  error.setAttribute("role", "status");
  const grid = document.createElement("div");
  grid.className = "art";
  grid.setAttribute("role", "img");
  const savedHeading = document.createElement("h3");
  savedHeading.textContent = "Saved art";
  const savedList = document.createElement("div");
  savedList.className = "saved";
  article.append(heading, controls, error, grid, savedHeading, savedList);
  context.root.append(style, article);

  const render = (state: TutorialState): void => {
    replaceOptions(algorithm.select, state.algorithms, state.algorithm);
    replaceOptions(palette.select, state.palettes, state.palette);
    error.textContent = state.error ?? "";
    save.disabled = !state.draft;
    grid.replaceChildren();
    if (state.draft) {
      const algorithmLabel = state.algorithms.find(({ id }) => id === state.draft?.recipe.algorithm)?.label ?? state.draft.recipe.algorithm;
      const paletteLabel = state.palettes.find(({ id }) => id === state.draft?.recipe.palette)?.label ?? state.draft.recipe.palette;
      grid.setAttribute("aria-label", `${algorithmLabel} artwork in the ${paletteLabel} palette`);
      for (const value of state.draft.cells) {
        const cell = document.createElement("span");
        cell.className = "cell";
        cell.setAttribute("aria-hidden", "true");
        cell.textContent = value;
        grid.append(cell);
      }
    } else {
      grid.setAttribute("aria-label", "Artwork has not loaded");
    }
    savedList.replaceChildren();
    for (const artwork of state.artworks) {
      const savedButton = button(`${labelFor(state.algorithms, artwork.recipe.algorithm)} · ${labelFor(state.palettes, artwork.recipe.palette)} · ${artwork.id}`);
      savedButton.setAttribute("aria-pressed", String(artwork.id === state.selectedArtworkId));
      savedButton.addEventListener("click", () => { controller.selectArtwork(artwork.id); });
      savedList.append(savedButton);
    }
  };
  const controller = createTutorialController(context.client, render);

  const onAlgorithm = (): void => { controller.setAlgorithm(algorithm.select.value); };
  const onPalette = (): void => { controller.setPalette(palette.select.value); };
  const onGenerate = (): void => { void controller.generate(); };
  const onSave = (): void => { void controller.save(); };
  algorithm.select.addEventListener("change", onAlgorithm);
  palette.select.addEventListener("change", onPalette);
  generate.addEventListener("click", onGenerate);
  save.addEventListener("click", onSave);
  await controller.start();

  let unmounted = false;
  return () => {
    if (unmounted) return;
    unmounted = true;
    controller.dispose();
    algorithm.select.removeEventListener("change", onAlgorithm);
    palette.select.removeEventListener("change", onPalette);
    generate.removeEventListener("click", onGenerate);
    save.removeEventListener("click", onSave);
    article.remove();
    style.remove();
  };
}

function selectControl(text: string): { label: HTMLLabelElement; select: HTMLSelectElement } {
  const label = document.createElement("label");
  label.append(text);
  const select = document.createElement("select");
  select.setAttribute("aria-label", text);
  label.append(select);
  return { label, select };
}

function replaceOptions(select: HTMLSelectElement, choices: readonly Choice[], selected: string): void {
  if (select.options.length !== choices.length) {
    select.replaceChildren(...choices.map((choice) => {
      const option = document.createElement("option");
      option.value = choice.id;
      option.textContent = choice.label;
      return option;
    }));
  }
  select.value = selected;
}

function button(text: string): HTMLButtonElement {
  const element = document.createElement("button");
  element.type = "button";
  element.textContent = text;
  return element;
}

function labelFor(choices: readonly Choice[], id: string): string {
  return choices.find((choice) => choice.id === id)?.label ?? id;
}
