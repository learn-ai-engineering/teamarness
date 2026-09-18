#![forbid(unsafe_code)]

use std::f64::consts::TAU;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use teamarness_package_protocol::JsonValue;
use teamarness_package_sdk::{BackendError, Context, Empty, Router};

pub const GENERATOR_VERSION: u32 = 1;
pub const GRID_WIDTH: u32 = 16;
pub const GRID_HEIGHT: u32 = 12;
const STATE_SCHEMA_VERSION: u32 = 2;
const STATE_FILE_NAME: &str = "tutorial.v1.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AlgorithmOption {
    pub id: &'static str,
    pub label: &'static str,
}

pub const ALGORITHMS: [AlgorithmOption; 4] = [
    AlgorithmOption {
        id: "waves",
        label: "Waves",
    },
    AlgorithmOption {
        id: "ripples",
        label: "Ripples",
    },
    AlgorithmOption {
        id: "spiral",
        label: "Spiral",
    },
    AlgorithmOption {
        id: "patchwork",
        label: "Patchwork",
    },
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PaletteOption {
    pub id: &'static str,
    pub label: &'static str,
    pub values: &'static [&'static str; PALETTE_SIZE],
}

const PALETTE_SIZE: usize = 5;
const GARDEN: [&str; PALETTE_SIZE] = ["🌱", "🌿", "🍀", "🌸", "🌻"];
const OCEAN: [&str; PALETTE_SIZE] = ["💧", "🐚", "🐠", "🐬", "🌊"];
const MOON: [&str; PALETTE_SIZE] = ["🌑", "🌒", "🌓", "🌔", "🌕"];
const FRUIT: [&str; PALETTE_SIZE] = ["🍓", "🍊", "🍋", "🍏", "🫐"];

pub const PALETTES: [PaletteOption; 4] = [
    PaletteOption {
        id: "garden",
        label: "Garden",
        values: &GARDEN,
    },
    PaletteOption {
        id: "ocean",
        label: "Ocean",
        values: &OCEAN,
    },
    PaletteOption {
        id: "moon",
        label: "Moon",
        values: &MOON,
    },
    PaletteOption {
        id: "fruit",
        label: "Fruit",
        values: &FRUIT,
    },
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Recipe {
    pub generator_version: u32,
    pub algorithm: String,
    pub palette: String,
    pub seed: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Draft {
    pub recipe: Recipe,
    pub cells: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SavedArtwork {
    pub id: String,
    pub recipe: Recipe,
    pub cells: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StateDocument {
    schema_version: u32,
    next_id: u64,
    artworks: Vec<SavedArtwork>,
}

impl Default for StateDocument {
    fn default() -> Self {
        Self {
            schema_version: STATE_SCHEMA_VERSION,
            next_id: 1,
            artworks: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TutorialError {
    code: &'static str,
}

impl TutorialError {
    #[must_use]
    pub const fn code(self) -> &'static str {
        self.code
    }
}

/// Renders one canonical recipe in row-major order.
pub fn render(recipe: &Recipe) -> Result<Vec<String>, TutorialError> {
    validate_recipe(recipe)?;
    let palette = palette_values(&recipe.palette)?;
    let mut cells = Vec::with_capacity((GRID_WIDTH * GRID_HEIGHT) as usize);

    for y in 0..GRID_HEIGHT {
        for x in 0..GRID_WIDTH {
            let index = match recipe.algorithm.as_str() {
                "waves" => waves_index(x, y, recipe.seed),
                "ripples" => ripples_index(x, y, recipe.seed),
                "spiral" => spiral_index(x, y, recipe.seed),
                "patchwork" => patchwork_index(x, y, recipe.seed),
                _ => return Err(error("tutorial.unknown_algorithm")),
            };
            cells.push(palette[index].to_owned());
        }
    }

    Ok(cells)
}

fn validate_recipe(recipe: &Recipe) -> Result<(), TutorialError> {
    if recipe.generator_version != GENERATOR_VERSION
        || recipe.width != GRID_WIDTH
        || recipe.height != GRID_HEIGHT
    {
        return Err(error("tutorial.invalid_recipe"));
    }
    if !ALGORITHMS
        .iter()
        .any(|option| option.id == recipe.algorithm)
    {
        return Err(error("tutorial.unknown_algorithm"));
    }
    palette_values(&recipe.palette)?;
    Ok(())
}

fn palette_values(id: &str) -> Result<&'static [&'static str; PALETTE_SIZE], TutorialError> {
    PALETTES
        .iter()
        .find(|palette| palette.id == id)
        .map(|palette| palette.values)
        .ok_or_else(|| error("tutorial.unknown_palette"))
}

fn waves_index(x: u32, y: u32, seed: u32) -> usize {
    let phase = f64::from(seed as u8) / 255.0 * TAU;
    let horizontal_frequency = f64::from(1 + ((seed >> 8) as u8 % 3));
    let vertical_frequency = f64::from(1 + ((seed >> 16) as u8 % 3));
    let horizontal =
        (f64::from(x) / f64::from(GRID_WIDTH - 1) * TAU * horizontal_frequency + phase).sin();
    let vertical =
        (f64::from(y) / f64::from(GRID_HEIGHT - 1) * TAU * vertical_frequency + phase).sin();
    palette_index((horizontal + vertical) / 2.0)
}

fn ripples_index(x: u32, y: u32, seed: u32) -> usize {
    let phase = f64::from(seed as u8) / 255.0 * TAU;
    let frequency = f64::from(2 + ((seed >> 8) as u8 % 3));
    let center_x = f64::from(GRID_WIDTH - 1) / 2.0;
    let center_y = f64::from(GRID_HEIGHT - 1) / 2.0;
    let distance = (f64::from(x) - center_x).hypot(f64::from(y) - center_y);
    palette_index((distance * frequency + phase).sin())
}

fn spiral_index(x: u32, y: u32, seed: u32) -> usize {
    let center_x = f64::from(GRID_WIDTH - 1) / 2.0;
    let center_y = f64::from(GRID_HEIGHT - 1) / 2.0;
    let dx = f64::from(x) - center_x;
    let dy = f64::from(y) - center_y;
    let distance = dx.hypot(dy) / center_x.hypot(center_y);
    let phase = f64::from(seed as u8) / 255.0 * TAU;
    let angle = dy.atan2(dx) + phase;
    palette_index((angle * 1.5 + distance * TAU * 2.0 + phase.sin()).sin())
}

fn patchwork_index(x: u32, y: u32, seed: u32) -> usize {
    let mut state = if seed == 0 { 0x9e37_79b9 } else { seed };
    let mut nearest = 0_usize;
    let mut nearest_distance = f64::INFINITY;
    for region in 0..PALETTE_SIZE {
        let point_x = f64::from(next_random(&mut state) % GRID_WIDTH);
        let point_y = f64::from(next_random(&mut state) % GRID_HEIGHT);
        let distance = (f64::from(x) - point_x).powi(2) + (f64::from(y) - point_y).powi(2);
        if distance < nearest_distance {
            nearest = region;
            nearest_distance = distance;
        }
    }
    nearest
}

fn next_random(state: &mut u32) -> u32 {
    *state ^= *state << 13;
    *state ^= *state >> 17;
    *state ^= *state << 5;
    *state
}

fn palette_index(value: f64) -> usize {
    ((((value + 1.0) / 2.0) * PALETTE_SIZE as f64).floor() as usize).min(PALETTE_SIZE - 1)
}

const fn error(code: &'static str) -> TutorialError {
    TutorialError { code }
}

#[derive(Debug)]
pub struct StateStore {
    directory: PathBuf,
    state: StateDocument,
}

impl StateStore {
    pub fn open(directory: impl AsRef<Path>) -> Result<Self, std::io::Error> {
        let directory = directory.as_ref().to_owned();
        std::fs::create_dir_all(&directory)?;
        let path = directory.join(STATE_FILE_NAME);
        let state = if path.exists() {
            let bytes = std::fs::read(&path)?;
            let mut state: StateDocument = serde_json::from_slice(&bytes).map_err(invalid_data)?;
            if migrate_state(&mut state)? {
                write_state(&directory, &state)?;
            }
            validate_state(&state)?;
            state
        } else {
            let state = StateDocument::default();
            write_state(&directory, &state)?;
            state
        };
        Ok(Self { directory, state })
    }

    #[must_use]
    pub fn artworks(&self) -> &[SavedArtwork] {
        &self.state.artworks
    }

    pub fn save(&mut self, recipe: Recipe) -> Result<SavedArtwork, StoreError> {
        let cells = render(&recipe).map_err(StoreError::Recipe)?;
        let artwork = SavedArtwork {
            id: format!("artwork-{}", self.state.next_id),
            recipe,
            cells,
        };
        let mut next = self.state.clone();
        next.next_id = next.next_id.checked_add(1).ok_or(StoreError::Storage)?;
        next.artworks.push(artwork.clone());
        write_state(&self.directory, &next).map_err(|_| StoreError::Storage)?;
        self.state = next;
        Ok(artwork)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreError {
    Recipe(TutorialError),
    Storage,
}

fn migrate_state(state: &mut StateDocument) -> Result<bool, std::io::Error> {
    if state.schema_version != 1 {
        return Ok(false);
    }
    if state.next_id == 0 {
        return Err(invalid_data("invalid Tutorial state version or next ID"));
    }
    for artwork in &mut state.artworks {
        if artwork.recipe.algorithm == "sierpinski" {
            artwork.recipe.algorithm = "spiral".to_owned();
        }
        if artwork.recipe.palette == "space" {
            artwork.recipe.palette = "moon".to_owned();
        }
        artwork.cells =
            render(&artwork.recipe).map_err(|_| invalid_data("invalid saved recipe"))?;
    }
    state.schema_version = STATE_SCHEMA_VERSION;
    Ok(true)
}

fn validate_state(state: &StateDocument) -> Result<(), std::io::Error> {
    if state.schema_version != STATE_SCHEMA_VERSION || state.next_id == 0 {
        return Err(invalid_data("invalid Tutorial state version or next ID"));
    }
    for artwork in &state.artworks {
        let expected = render(&artwork.recipe).map_err(|_| invalid_data("invalid saved recipe"))?;
        if artwork.cells != expected {
            return Err(invalid_data("saved cells do not match recipe"));
        }
    }
    Ok(())
}

/// Replaces the state document, leaving the previous one intact on any failure.
///
/// The SDK's helper flushes the bytes and the directory entry before the rename, so a crash
/// never leaves a half-written document behind. Its temporary name is `tutorial.v1.json.tmp`.
fn write_state(directory: &Path, state: &StateDocument) -> Result<(), std::io::Error> {
    let mut bytes = serde_json::to_vec_pretty(state).map_err(invalid_data)?;
    bytes.push(b'\n');
    teamarness_package_sdk::storage::write_atomic(&directory.join(STATE_FILE_NAME), &bytes)
}

fn invalid_data(error: impl std::fmt::Display) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, error.to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeedError;

pub trait SeedSource: Send + Sync + 'static {
    fn next_seed(&self) -> Result<u32, SeedError>;
}

#[derive(Debug, Default)]
pub struct SystemSeed;

impl SeedSource for SystemSeed {
    fn next_seed(&self) -> Result<u32, SeedError> {
        let mut bytes = [0_u8; 4];
        getrandom::fill(&mut bytes).map_err(|_| SeedError)?;
        Ok(u32::from_le_bytes(bytes))
    }
}

/// Everything one activation of the Tutorial owns.
///
/// The store is a plain synchronous mutex: every handler touches it inside `Context::blocking`,
/// so file work never holds an async worker.
pub struct TutorialState {
    store: Mutex<StateStore>,
    seeds: Arc<dyn SeedSource>,
}

/// The Tutorial Backend, with the platform's own source of randomness.
#[must_use]
pub fn router() -> Router<TutorialState> {
    router_with_seed_source(SystemSeed)
}

/// The Tutorial Backend with a chosen source of seeds, which is what tests use.
#[must_use]
pub fn router_with_seed_source<S: SeedSource>(seeds: S) -> Router<TutorialState> {
    let seeds: Arc<dyn SeedSource> = Arc::new(seeds);
    Router::new(move |activation| {
        let seeds = seeds.clone();
        let directory = activation.data_directory().to_owned();
        let cancellation = activation.cancellation().clone();
        async move {
            let store = teamarness_package_sdk::blocking(&cancellation, move |_| {
                StateStore::open(&directory).map_err(|_| storage_failed())
            })
            .await?;
            Ok(TutorialState {
                store: Mutex::new(store),
                seeds,
            })
        }
    })
    .request(
        "tutorial.read",
        |context: Context<TutorialState>, _: Empty| async move {
            let state = context.state().clone();
            context
                .blocking(move |_| {
                    let store = state.store.lock().map_err(|_| storage_failed())?;
                    serde_json::to_value(ReadResponse {
                        algorithms: &ALGORITHMS,
                        palettes: &PALETTES,
                        artworks: store.artworks(),
                    })
                    .map_err(|_| storage_failed())
                })
                .await
        },
    )
    .request(
        "tutorial.generate",
        |context: Context<TutorialState>, request: GenerateRequest| async move {
            let state = context.state().clone();
            context
                .blocking(move |_| {
                    let mut recipe = Recipe {
                        generator_version: GENERATOR_VERSION,
                        algorithm: request.algorithm,
                        palette: request.palette,
                        seed: 0,
                        width: GRID_WIDTH,
                        height: GRID_HEIGHT,
                    };
                    validate_recipe(&recipe).map_err(domain_backend_error)?;
                    recipe.seed = state
                        .seeds
                        .next_seed()
                        .map_err(|SeedError| backend_error("tutorial.random_failed", true))?;
                    let cells = render(&recipe).map_err(domain_backend_error)?;
                    serde_json::to_value(Draft { recipe, cells }).map_err(|_| storage_failed())
                })
                .await
        },
    )
    .request(
        "tutorial.save",
        |context: Context<TutorialState>, request: SaveRequest| async move {
            let state = context.state().clone();
            context
                .blocking(move |_| {
                    let mut store = state.store.lock().map_err(|_| storage_failed())?;
                    let artwork = store.save(request.recipe).map_err(store_backend_error)?;
                    serde_json::to_value(artwork).map_err(|_| storage_failed())
                })
                .await
        },
    )
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ReadResponse<'a> {
    algorithms: &'a [AlgorithmOption],
    palettes: &'a [PaletteOption],
    artworks: &'a [SavedArtwork],
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GenerateRequest {
    algorithm: String,
    palette: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SaveRequest {
    recipe: Recipe,
}

fn store_backend_error(error: StoreError) -> BackendError {
    match error {
        StoreError::Recipe(error) => domain_backend_error(error),
        StoreError::Storage => backend_error("tutorial.storage_failed", true),
    }
}

fn domain_backend_error(error: TutorialError) -> BackendError {
    backend_error(error.code(), false)
}

fn backend_error(code: &'static str, retryable: bool) -> BackendError {
    BackendError::new(code, retryable, JsonValue::Object(Default::default()))
}

/// A poisoned mutex or a failed write is the same thing to the UI: storage did not work.
fn storage_failed() -> BackendError {
    backend_error("tutorial.storage_failed", true)
}
