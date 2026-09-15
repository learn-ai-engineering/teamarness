#![forbid(unsafe_code)]

use std::f64::consts::TAU;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use teamarness_package_protocol::JsonValue;
use teamarness_package_sdk::{Backend, BackendError, BackendRequest, EventSubscription, Package};
use tokio::sync::Mutex;

pub const GENERATOR_VERSION: u32 = 1;
pub const GRID_WIDTH: u32 = 16;
pub const GRID_HEIGHT: u32 = 12;
const STATE_SCHEMA_VERSION: u32 = 1;
const STATE_FILE_NAME: &str = "tutorial.v1.json";
const TEMP_FILE_NAME: &str = "tutorial.v1.json.tmp";

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
    pub values: &'static [&'static str; 8],
}

const GARDEN: [&str; 8] = ["🌱", "🌿", "🍀", "🌸", "🌻", "🪻", "🌷", "🐝"];
const OCEAN: [&str; 8] = ["💧", "🐚", "🐠", "🐬", "🌊", "🪼", "🐙", "🐳"];
const SPACE: [&str; 8] = ["🌑", "🌒", "🌓", "🌔", "🌕", "⭐", "🪐", "🚀"];
const FRUIT: [&str; 8] = ["🍓", "🍊", "🍋", "🍏", "🫐", "🍇", "🍉", "🍒"];

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
        id: "space",
        label: "Space",
        values: &SPACE,
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

fn palette_values(id: &str) -> Result<&'static [&'static str; 8], TutorialError> {
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
    let mut state = seed;
    let mut nearest = 0_usize;
    let mut nearest_distance = f64::INFINITY;
    for region in 0..8_usize {
        let point_x = f64::from((region as u32 % 4) * 4 + next_random(&mut state) % 4);
        let point_y = f64::from((region as u32 / 4) * 6 + next_random(&mut state) % 6);
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
    ((((value + 1.0) / 2.0) * 8.0).floor() as usize).min(7)
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
            let state: StateDocument = serde_json::from_slice(&bytes).map_err(invalid_data)?;
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

fn write_state(directory: &Path, state: &StateDocument) -> Result<(), std::io::Error> {
    let temporary = directory.join(TEMP_FILE_NAME);
    let destination = directory.join(STATE_FILE_NAME);
    let bytes = serde_json::to_vec_pretty(state).map_err(invalid_data)?;
    std::fs::write(&temporary, bytes)?;
    if let Err(write_error) = std::fs::rename(&temporary, &destination) {
        let _ = std::fs::remove_file(&temporary);
        return Err(write_error);
    }
    Ok(())
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

pub struct TutorialBackend<S = SystemSeed> {
    active: Arc<Mutex<Option<StateStore>>>,
    seeds: S,
}

impl Default for TutorialBackend<SystemSeed> {
    fn default() -> Self {
        Self::with_seed_source(SystemSeed)
    }
}

impl<S: SeedSource> TutorialBackend<S> {
    #[must_use]
    pub fn with_seed_source(seeds: S) -> Self {
        Self {
            active: Arc::new(Mutex::new(None)),
            seeds,
        }
    }

    pub async fn serve(self) -> Result<(), teamarness_package_sdk::SdkError> {
        Package::builder(Arc::new(self)).serve().await
    }

    async fn activate(&self, request: &BackendRequest) -> Result<JsonValue, BackendError> {
        let path = request
            .initialization
            .data_directory()
            .get("path")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| backend_error("tutorial.initialization_invalid", false))?;
        let store =
            StateStore::open(path).map_err(|_| backend_error("tutorial.storage_failed", true))?;
        *self.active.lock().await = Some(store);
        Ok(empty_object())
    }

    async fn read(&self) -> Result<JsonValue, BackendError> {
        let active = self.active.lock().await;
        let store = active
            .as_ref()
            .ok_or_else(|| backend_error("tutorial.not_active", true))?;
        serde_json::to_value(ReadResponse {
            algorithms: &ALGORITHMS,
            palettes: &PALETTES,
            artworks: store.artworks(),
        })
        .map_err(|_| backend_error("tutorial.storage_failed", true))
    }

    async fn generate(&self, payload: JsonValue) -> Result<JsonValue, BackendError> {
        if self.active.lock().await.is_none() {
            return Err(backend_error("tutorial.not_active", true));
        }
        let request: GenerateRequest = serde_json::from_value(payload)
            .map_err(|_| backend_error("tutorial.invalid_request", false))?;
        let mut recipe = Recipe {
            generator_version: GENERATOR_VERSION,
            algorithm: request.algorithm,
            palette: request.palette,
            seed: 0,
            width: GRID_WIDTH,
            height: GRID_HEIGHT,
        };
        validate_recipe(&recipe).map_err(domain_backend_error)?;
        recipe.seed = self
            .seeds
            .next_seed()
            .map_err(|SeedError| backend_error("tutorial.random_failed", true))?;
        let cells = render(&recipe).map_err(domain_backend_error)?;
        serde_json::to_value(Draft { recipe, cells })
            .map_err(|_| backend_error("tutorial.storage_failed", true))
    }

    async fn save(&self, payload: JsonValue) -> Result<JsonValue, BackendError> {
        let request: SaveRequest = serde_json::from_value(payload)
            .map_err(|_| backend_error("tutorial.invalid_request", false))?;
        let mut active = self.active.lock().await;
        let store = active
            .as_mut()
            .ok_or_else(|| backend_error("tutorial.not_active", true))?;
        let artwork = store.save(request.recipe).map_err(store_backend_error)?;
        serde_json::to_value(artwork).map_err(|_| backend_error("tutorial.storage_failed", true))
    }
}

impl<S: SeedSource> Backend for TutorialBackend<S> {
    fn handle(
        &self,
        request: BackendRequest,
    ) -> Pin<Box<dyn Future<Output = Result<JsonValue, BackendError>> + Send + '_>> {
        Box::pin(async move {
            match request.operation.as_str() {
                "lifecycle.activate" | "lifecycle.recover" => self.activate(&request).await,
                "lifecycle.suspend" => Ok(empty_object()),
                "lifecycle.dispose" => {
                    *self.active.lock().await = None;
                    Ok(empty_object())
                }
                "tutorial.read" => {
                    parse_empty(request.payload)?;
                    self.read().await
                }
                "tutorial.generate" => self.generate(request.payload).await,
                "tutorial.save" => self.save(request.payload).await,
                _ => Err(backend_error("tutorial.unknown_operation", false)),
            }
        })
    }

    fn subscribe(
        &self,
        _request: BackendRequest,
    ) -> Pin<Box<dyn Future<Output = Result<EventSubscription, BackendError>> + Send + '_>> {
        Box::pin(async { Err(backend_error("tutorial.unknown_subscription", false)) })
    }
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

fn parse_empty(payload: JsonValue) -> Result<(), BackendError> {
    let fields = payload
        .as_object()
        .ok_or_else(|| backend_error("tutorial.invalid_request", false))?;
    if fields.is_empty() {
        Ok(())
    } else {
        Err(backend_error("tutorial.invalid_request", false))
    }
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
    BackendError::new(code, retryable, empty_object())
}

fn empty_object() -> JsonValue {
    JsonValue::Object(Default::default())
}
