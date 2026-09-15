use teamarness_tutorial_package::{GRID_HEIGHT, GRID_WIDTH, Recipe, render};

fn recipe(algorithm: &str, palette: &str, seed: u32) -> Recipe {
    Recipe {
        generator_version: 1,
        algorithm: algorithm.to_owned(),
        palette: palette.to_owned(),
        seed,
        width: GRID_WIDTH,
        height: GRID_HEIGHT,
    }
}

#[test]
fn renders_independent_examples_for_each_algorithm() {
    let waves = render(&recipe("waves", "garden", 0x1234_5678)).unwrap();
    assert_eq!(
        &waves[..16],
        [
            "🍀", "🌿", "🌿", "🌸", "🌸", "🍀", "🌿", "🌿", "🌸", "🌸", "🍀", "🌿", "🌿", "🌸",
            "🌸", "🍀"
        ]
    );

    let ripples = render(&recipe("ripples", "ocean", 0x1234_5678)).unwrap();
    assert_eq!(
        &ripples[..16],
        [
            "🌊", "💧", "🐬", "🐠", "💧", "🌊", "🐬", "🐠", "🐠", "🐬", "🌊", "💧", "🐠", "🐬",
            "💧", "🌊"
        ]
    );

    let sierpinski = render(&recipe("sierpinski", "fruit", 0x1234_5678)).unwrap();
    assert_eq!(
        &sierpinski[..16],
        [
            "🍏", "🫐", "🍊", "🍋", "🍏", "🫐", "🍊", "🍋", "🍏", "🫐", "🍊", "🍋", "🍏", "🫐",
            "🍊", "🍋"
        ]
    );
}

#[test]
fn recipes_are_reproducible_bounded_and_seed_sensitive() {
    for algorithm in ["waves", "ripples", "sierpinski"] {
        let first = render(&recipe(algorithm, "space", 7)).unwrap();
        assert_eq!(first, render(&recipe(algorithm, "space", 7)).unwrap());
        assert_ne!(first, render(&recipe(algorithm, "space", 91)).unwrap());
        assert_eq!(first.len(), 192);
        assert!(
            first
                .iter()
                .all(|cell| ["🌑", "🌒", "🌓", "🌔", "🌕"].contains(&cell.as_str()))
        );
    }
}

#[test]
fn recipes_reject_unknown_choices_and_noncanonical_versions_or_dimensions() {
    let unknown_algorithm = render(&recipe("spiral", "garden", 1)).unwrap_err();
    assert_eq!(unknown_algorithm.code(), "tutorial.unknown_algorithm");

    let unknown_palette = render(&recipe("waves", "desert", 1)).unwrap_err();
    assert_eq!(unknown_palette.code(), "tutorial.unknown_palette");

    let mut wrong_version = recipe("waves", "garden", 1);
    wrong_version.generator_version = 2;
    assert_eq!(
        render(&wrong_version).unwrap_err().code(),
        "tutorial.invalid_recipe"
    );

    let mut wrong_dimensions = recipe("waves", "garden", 1);
    wrong_dimensions.width = 20;
    assert_eq!(
        render(&wrong_dimensions).unwrap_err().code(),
        "tutorial.invalid_recipe"
    );
}
