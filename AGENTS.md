## Project Summary

This is a small Rust espresso dial-in application for tracking recipes, shot
samples, grinder-setting predictions, and SVG graphs. It has both CLI commands
and an internal phone/tablet-friendly web UI. The Bash entrypoint `./coffee.sh`
remains the supported user-facing command and runs the Rust app through Cargo.

A recipe has a fixed numeric dose in grams and fixed numeric shot weight in
grams. Shot samples record the pulled shot time and numeric grinder setting used
for that shot. The grinder setting is the variable adjusted to reach a target
shot time; valid grinder settings are from `1` (finest) to `40` (very coarse).

The application stores data in `coffee_recipes.tsv`. Use `./coffee.sh` rather
than editing the TSV directly unless a migration or targeted data cleanup is
needed.

## Capabilities

- Manage recipes with fixed dose and shot weights.
- Add shot samples with shot time and grinder setting.
- Remove recipes and individual shot samples after confirmation in the web UI.
- Predict the grinder setting needed for a target shot time.
- Use a local Theil-Sen regression model for predictions. The model uses up to
  the 6 numeric samples closest to the target shot time, which reduces the
  influence of distant or poor samples.
- Render dark-themed SVG graphs of shot time vs grind.
- Graphs show all numeric samples, highlight the local samples used for the
  model, draw the local Theil-Sen predictive line, and mark the target shot time
  and predicted grind. The graph no longer includes an in-plot legend.
- Graph shot time is fixed from `0s` to `60s`. The grind axis adapts to the
  smallest and largest sample grinds for the recipe to avoid wasted space.
- Graph generation requires at least two numeric samples with varying grind
  settings.
- Serve an internal web UI on all interfaces by default at port `9000`.
- The web UI is the primary mobile/tablet workflow: select or create recipes,
  add samples, delete samples, delete recipes, click the predicted next grind to
  prefill the next sample grind, and immediately see updated prediction and SVG
  graph state.

## Tools

Run the internal web UI:

```bash
./coffee.sh serve
```

Override host or port:

```bash
./coffee.sh serve --host 127.0.0.1 --port 9001
```

List recipes in a parseable block format:

```bash
./coffee.sh recipes
```

Add a recipe with its fixed dose and shot weight:

```bash
./coffee.sh add --recipe RECIPE --dose DOSE_WEIGHT_G --shot-weight SHOT_WEIGHT_G
```

Add a shot sample to a recipe:

```bash
./coffee.sh sample --recipe RECIPE --time SHOT_TIME --grind GRIND
```

Predict the grinder setting for a target shot time:

```bash
./coffee.sh predict --recipe RECIPE --time TARGET_SHOT_TIME
```

Render an SVG graph for a target shot time:

```bash
./coffee.sh graph --recipe RECIPE --time TARGET_SHOT_TIME --output graph.svg
```

Remove a recipe and all of its samples:

```bash
./coffee.sh remove --recipe RECIPE
```

Recipes and samples are stored in `coffee_recipes.tsv` with this schema:

```text
record_type<TAB>recipe<TAB>dose_weight_g<TAB>shot_weight_g<TAB>time<TAB>grind
```

Rows with `record_type` of `recipe` define the recipe and fixed `dose_weight_g`
and `shot_weight_g`. Rows with `record_type` of `sample` define shot samples for
a recipe using `time` and numeric `grind`.

This is a prototype. The application expects the current TSV schema and does
not maintain legacy data-schema migrations.
