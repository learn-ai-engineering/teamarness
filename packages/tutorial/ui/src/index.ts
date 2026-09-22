import {
  adoptPackageStyles,
  describePackageError,
  typedPackageClient,
  type PackageClient,
  type PackageMountContext,
  type PackageUnmount,
} from "@teamarness/package-ui-sdk";
import type {
  AlgorithmChoice,
  AlgorithmId,
  Draft,
  PaletteChoice,
  PaletteId,
  Recipe,
  SavedArtwork,
  TutorialContract,
} from "./contract.generated";

export type { AlgorithmChoice, AlgorithmId, Draft, PaletteChoice, PaletteId, Recipe, SavedArtwork };

/** What the controls need from any contract choice: something to select and something to show. */
export type Choice = Readonly<{ id: string; label: string }>;

const TUTORIAL_PACKAGE_CSS = `
  :host { color: var(--teamarness-color-text); font-family: var(--teamarness-font-body); }
  button { cursor: pointer; }
  .art { display: grid; grid-template-columns: repeat(16, 1.75rem); grid-auto-rows: 1.75rem; width: fit-content; max-width: 100%; overflow: auto; }
  .cell { display: grid; place-items: center; font-size: 1.25rem; line-height: 1; }
  .error { margin: 0; color: var(--teamarness-color-danger, #b42318); min-height: 1.2em; }
  .saved button[aria-pressed="true"] { outline: 2px solid currentColor; }
`;

export type TutorialState = Readonly<{
  algorithms: readonly AlgorithmChoice[];
  palettes: readonly PaletteChoice[];
  artworks: readonly SavedArtwork[];
  algorithm: AlgorithmId | "";
  palette: PaletteId | "";
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
  const typed = typedPackageClient<TutorialContract>(client);
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
    update({ error: describePackageError(error) });
  };
  const generate = async (): Promise<void> => {
    const { algorithm, palette } = state;
    if (disposed || !algorithm || !palette) return;
    try {
      const result = await typed.request("tutorial.generate", { algorithm, palette });
      update({ draft: result, selectedArtworkId: undefined, error: undefined });
    } catch (error) {
      fail(error);
    }
  };

  return Object.freeze({
    async start(): Promise<void> {
      if (disposed) return;
      try {
        const result = await typed.request("tutorial.read", {});
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
      const choice = state.algorithms.find(({ id }) => id === value);
      if (choice) update({ algorithm: choice.id, selectedArtworkId: undefined });
    },
    setPalette(value: string): void {
      const choice = state.palettes.find(({ id }) => id === value);
      if (choice) update({ palette: choice.id, selectedArtworkId: undefined });
    },
    generate,
    async save(): Promise<void> {
      if (disposed || !state.draft) return;
      try {
        const saved = await typed.request("tutorial.save", { recipe: state.draft.recipe });
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
  const cleanupStyles = adoptPackageStyles(context.root, TUTORIAL_PACKAGE_CSS);
  const article = document.createElement("article");
  article.className = "teamarness-page";
  const header = document.createElement("header");
  header.className = "teamarness-page-header";
  const heading = document.createElement("h1");
  heading.className = "teamarness-page-title";
  heading.textContent = context.package.displayName;
  const subtitle = document.createElement("p");
  subtitle.className = "teamarness-page-subtitle";
  subtitle.textContent = "Generate emoji art and save the recipes you like.";
  header.append(heading, subtitle);
  const artSection = document.createElement("section");
  artSection.className = "teamarness-section teamarness-stack";
  const controls = document.createElement("div");
  controls.className = "controls teamarness-inline";
  const palette = selectControl("Palette");
  const algorithm = selectControl("Pattern");
  const generate = button("Generate", "primary");
  const save = button("Save", "secondary");
  controls.append(palette, algorithm, generate, save);
  const error = document.createElement("p");
  error.className = "error";
  error.setAttribute("role", "status");
  const grid = document.createElement("div");
  grid.className = "art";
  grid.setAttribute("role", "img");
  artSection.append(controls, error, grid);
  const savedSection = document.createElement("section");
  savedSection.className = "teamarness-section";
  const savedHeading = document.createElement("h2");
  savedHeading.className = "teamarness-section-title";
  savedHeading.textContent = "Saved art";
  const savedList = document.createElement("div");
  savedList.className = "saved teamarness-inline";
  savedSection.append(savedHeading, savedList);
  article.append(header, artSection, savedSection);
  context.root.append(article);

  const render = (state: TutorialState): void => {
    replaceOptions(palette, state.palettes, state.palette);
    replaceOptions(algorithm, state.algorithms, state.algorithm);
    error.textContent = state.error ?? "";
    save.disabled = !state.draft;
    grid.replaceChildren();
    if (state.draft) {
      const algorithmLabel = state.algorithms.find(({ id }) => id === state.draft?.recipe.algorithm)?.label ?? state.draft.recipe.algorithm;
      const paletteLabel = state.palettes.find(({ id }) => id === state.draft?.recipe.palette)?.label ?? state.draft.recipe.palette;
      grid.setAttribute("aria-label", `${paletteLabel} ${algorithmLabel} artwork`);
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
      const savedButton = button(`${labelFor(state.palettes, artwork.recipe.palette)} ${labelFor(state.algorithms, artwork.recipe.algorithm)} · ${artwork.id}`, "secondary");
      savedButton.setAttribute("aria-pressed", String(artwork.id === state.selectedArtworkId));
      savedButton.addEventListener("click", () => { controller.selectArtwork(artwork.id); });
      savedList.append(savedButton);
    }
  };
  const controller = createTutorialController(context.client, render);

  const onAlgorithm = (): void => { controller.setAlgorithm(algorithm.value); };
  const onPalette = (): void => { controller.setPalette(palette.value); };
  const onGenerate = (): void => { void controller.generate(); };
  const onSave = (): void => { void controller.save(); };
  algorithm.addEventListener("change", onAlgorithm);
  palette.addEventListener("change", onPalette);
  generate.addEventListener("click", onGenerate);
  save.addEventListener("click", onSave);
  await controller.start();

  let unmounted = false;
  return () => {
    if (unmounted) return;
    unmounted = true;
    controller.dispose();
    algorithm.removeEventListener("change", onAlgorithm);
    palette.removeEventListener("change", onPalette);
    generate.removeEventListener("click", onGenerate);
    save.removeEventListener("click", onSave);
    article.remove();
    cleanupStyles();
    cleanupKit();
  };
}

function selectControl(accessibleName: string): HTMLSelectElement {
  const select = document.createElement("select");
  select.className = "teamarness-control";
  select.setAttribute("aria-label", accessibleName);
  return select;
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

function button(text: string, variant: "primary" | "secondary"): HTMLButtonElement {
  const element = document.createElement("button");
  element.type = "button";
  element.className = `teamarness-control teamarness-control--${variant}`;
  element.textContent = text;
  return element;
}

function labelFor(choices: readonly Choice[], id: string): string {
  return choices.find((choice) => choice.id === id)?.label ?? id;
}
