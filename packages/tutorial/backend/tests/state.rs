use std::fs;

use teamarness_tutorial_package::{GRID_HEIGHT, GRID_WIDTH, Recipe, StateStore, StoreError};

fn recipe(seed: u32) -> Recipe {
    Recipe {
        generator_version: 1,
        algorithm: "waves".to_owned(),
        palette: "garden".to_owned(),
        seed,
        width: GRID_WIDTH,
        height: GRID_HEIGHT,
    }
}

#[test]
fn open_initializes_state_and_save_allocates_stable_ids_across_reopen() {
    let root = tempfile::tempdir().unwrap();
    let mut store = StateStore::open(root.path()).unwrap();
    assert!(store.artworks().is_empty());
    assert!(root.path().join("tutorial.v1.json").is_file());

    let first = store.save(recipe(7)).unwrap();
    let second = store.save(recipe(91)).unwrap();
    assert_eq!(first.id, "artwork-1");
    assert_eq!(second.id, "artwork-2");
    drop(store);

    let mut reopened = StateStore::open(root.path()).unwrap();
    assert_eq!(reopened.artworks(), [first, second]);
    assert_eq!(reopened.save(recipe(137)).unwrap().id, "artwork-3");
}

#[test]
fn open_migrates_version_one_saved_art_to_current_recipes() {
    let root = tempfile::tempdir().unwrap();
    let old_state = serde_json::json!({
        "schemaVersion": 1,
        "nextId": 2,
        "artworks": [{
            "id": "artwork-1",
            "recipe": {
                "generatorVersion": 1,
                "algorithm": "sierpinski",
                "palette": "space",
                "seed": 7,
                "width": 16,
                "height": 12
            },
            "cells": vec!["🚀"; 192]
        }]
    });
    fs::write(
        root.path().join("tutorial.v1.json"),
        serde_json::to_vec_pretty(&old_state).unwrap(),
    )
    .unwrap();

    let store = StateStore::open(root.path()).unwrap();
    let migrated = &store.artworks()[0];
    assert_eq!(migrated.id, "artwork-1");
    assert_eq!(migrated.recipe.algorithm, "spiral");
    assert_eq!(migrated.recipe.palette, "moon");
    assert_eq!(migrated.recipe.seed, 7);
    assert_eq!(migrated.cells.len(), 192);
    assert!(
        migrated
            .cells
            .iter()
            .all(|cell| ["🌑", "🌒", "🌓", "🌔", "🌕"].contains(&cell.as_str()))
    );

    let saved: serde_json::Value =
        serde_json::from_slice(&fs::read(root.path().join("tutorial.v1.json")).unwrap()).unwrap();
    assert_eq!(saved["schemaVersion"], 2);
    assert_eq!(saved["artworks"][0]["id"], "artwork-1");
    assert_eq!(saved["artworks"][0]["recipe"]["algorithm"], "spiral");
    assert_eq!(saved["artworks"][0]["recipe"]["palette"], "moon");
    assert_eq!(saved["artworks"][0]["recipe"]["seed"], 7);
}

#[test]
fn failed_atomic_replacement_preserves_the_last_complete_state() {
    let root = tempfile::tempdir().unwrap();
    let mut store = StateStore::open(root.path()).unwrap();
    let first = store.save(recipe(7)).unwrap();

    let temporary_path = root.path().join("tutorial.v1.json.tmp");
    fs::create_dir(&temporary_path).unwrap();
    assert_eq!(store.save(recipe(91)), Err(StoreError::Storage));

    let reopened = StateStore::open(root.path()).unwrap();
    assert_eq!(reopened.artworks(), [first]);

    fs::remove_dir(&temporary_path).unwrap();
    assert_eq!(store.save(recipe(137)).unwrap().id, "artwork-2");
}

#[test]
fn save_validates_the_recipe_before_changing_state() {
    let root = tempfile::tempdir().unwrap();
    let mut store = StateStore::open(root.path()).unwrap();
    let mut invalid = recipe(7);
    invalid.palette = "unknown".to_owned();

    let error = store.save(invalid).unwrap_err();
    let StoreError::Recipe(error) = error else {
        panic!("expected recipe error");
    };
    assert_eq!(error.code(), "tutorial.unknown_palette");
    assert!(store.artworks().is_empty());
}
