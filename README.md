# Coffee Hit

This is a small, vibe-coded espresso dial-in app. It is probably terrible, but
it exists for my own personal excuse: tracking coffee recipes, shot samples, and
rough grinder-setting predictions without pretending this is a serious product.

It stores recipes and shot samples locally, predicts a grind setting for a target
shot time, and can show simple SVG graphs. The supported way to run it is through
the Bash wrapper:

```bash
./coffee.sh serve
```

There are also CLI commands for adding recipes, recording samples, predicting the
next grind, and rendering graphs. Data lives in `coffee_recipes.tsv`.
