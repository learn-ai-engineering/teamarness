// Generated from ui-contract.json by teamarness-contract-types. Do not edit.

export type AlgorithmChoice = {
  readonly "id": "waves";
  readonly "label": "Waves";
} | {
  readonly "id": "ripples";
  readonly "label": "Ripples";
} | {
  readonly "id": "spiral";
  readonly "label": "Spiral";
} | {
  readonly "id": "patchwork";
  readonly "label": "Patchwork";
};

export type AlgorithmId = "waves" | "ripples" | "spiral" | "patchwork";

export type Cells = readonly ("🌱" | "🌿" | "🍀" | "🌸" | "🌻" | "💧" | "🐚" | "🐠" | "🐬" | "🌊" | "🌑" | "🌒" | "🌓" | "🌔" | "🌕" | "🍓" | "🍊" | "🍋" | "🍏" | "🫐")[];

export type Draft = {
  readonly "cells": Cells;
  readonly "recipe": Recipe;
};

export type Empty = Record<string, never>;

export type GenerateRequest = {
  readonly "algorithm": AlgorithmId;
  readonly "palette": PaletteId;
};

export type PaletteChoice = {
  readonly "id": "garden";
  readonly "label": "Garden";
  readonly "values": readonly ["🌱", "🌿", "🍀", "🌸", "🌻"];
} | {
  readonly "id": "ocean";
  readonly "label": "Ocean";
  readonly "values": readonly ["💧", "🐚", "🐠", "🐬", "🌊"];
} | {
  readonly "id": "moon";
  readonly "label": "Moon";
  readonly "values": readonly ["🌑", "🌒", "🌓", "🌔", "🌕"];
} | {
  readonly "id": "fruit";
  readonly "label": "Fruit";
  readonly "values": readonly ["🍓", "🍊", "🍋", "🍏", "🫐"];
};

export type PaletteId = "garden" | "ocean" | "moon" | "fruit";

export type ReadResponse = {
  readonly "algorithms": readonly AlgorithmChoice[];
  readonly "artworks": readonly SavedArtwork[];
  readonly "palettes": readonly PaletteChoice[];
};

export type Recipe = {
  readonly "algorithm": AlgorithmId;
  readonly "generatorVersion": 1;
  readonly "height": 12;
  readonly "palette": PaletteId;
  readonly "seed": number;
  readonly "width": 16;
};

export type SaveRequest = {
  readonly "recipe": Recipe;
};

export type SavedArtwork = {
  readonly "cells": Cells;
  readonly "id": string;
  readonly "recipe": Recipe;
};

export type TutorialContract = {
  readonly requests: {
    readonly "tutorial.generate": { readonly request: GenerateRequest; readonly response: Draft };
    readonly "tutorial.read": { readonly request: Empty; readonly response: ReadResponse };
    readonly "tutorial.save": { readonly request: SaveRequest; readonly response: SavedArtwork };
  };
  readonly subscriptions: Record<string, never>;
};
