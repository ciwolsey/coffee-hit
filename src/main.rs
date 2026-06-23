use std::env;
use std::error::Error;
use std::fmt::{self, Display};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process;

const HEADER: &str = "record_type\trecipe\tdose_weight_g\ttime\tgrind\n";
const LOCAL_MODEL_SAMPLE_LIMIT: usize = 6;

#[derive(Debug)]
struct AppError(String);

impl AppError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Error for AppError {}

#[derive(Clone, Debug)]
struct Recipe {
    name: String,
    dose_weight_g: String,
    samples: Vec<Sample>,
}

#[derive(Clone, Debug)]
struct Sample {
    recipe: String,
    time: String,
    grind: String,
}

#[derive(Default)]
struct Data {
    recipes: Vec<Recipe>,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let mut args: Vec<String> = env::args().skip(1).collect();
    let command = if args.is_empty() {
        "recipes".to_string()
    } else {
        args.remove(0)
    };

    let data_file = data_file_path()?;

    match command.as_str() {
        "recipes" | "list" => list_recipes(&data_file),
        "add" => add_recipe(&data_file, &args),
        "sample" | "add-sample" => add_sample(&data_file, &args),
        "predict" => predict_recipe(&data_file, &args),
        "graph" => graph_recipe(&data_file, &args),
        "remove" | "rm" => remove_recipe(&data_file, &args),
        "-h" | "--help" | "help" => {
            print_usage();
            Ok(())
        }
        _ => {
            print_usage_to_stderr();
            Err(Box::new(AppError::new(format!(
                "Unknown command: {command}"
            ))))
        }
    }
}

fn data_file_path() -> Result<PathBuf, Box<dyn Error>> {
    let exe = env::current_exe()?;
    let exe_dir = exe
        .parent()
        .ok_or_else(|| AppError::new("Cannot determine executable directory"))?;

    if exe_dir
        .file_name()
        .is_some_and(|name| name == "debug" || name == "release")
    {
        if let Some(target_dir) = exe_dir.parent() {
            if target_dir.file_name().is_some_and(|name| name == "target") {
                if let Some(project_dir) = target_dir.parent() {
                    return Ok(project_dir.join("coffee_recipes.tsv"));
                }
            }
        }
    }

    Ok(exe_dir.join("coffee_recipes.tsv"))
}

fn print_usage() {
    println!(
        "Usage:
  ./coffee.sh recipes
  ./coffee.sh add --recipe RECIPE --dose DOSE_WEIGHT_G
  ./coffee.sh sample --recipe RECIPE --time SHOT_TIME --grind GRIND
  ./coffee.sh predict --recipe RECIPE --time TARGET_SHOT_TIME
  ./coffee.sh graph --recipe RECIPE --time TARGET_SHOT_TIME [--output graph.svg]
  ./coffee.sh remove --recipe RECIPE

Recipes are stored in coffee_recipes.tsv as:
  record_type<TAB>recipe<TAB>dose_weight_g<TAB>time<TAB>grind

Rows with record_type \"recipe\" define recipes and their fixed dose in grams.
Rows with record_type \"sample\" define shot samples for a recipe. Grind is a
numeric grinder setting from 1 (finest) to 40 (very coarse)."
    );
}

fn print_usage_to_stderr() {
    eprintln!(
        "Usage:
  ./coffee.sh recipes
  ./coffee.sh add --recipe RECIPE --dose DOSE_WEIGHT_G
  ./coffee.sh sample --recipe RECIPE --time SHOT_TIME --grind GRIND
  ./coffee.sh predict --recipe RECIPE --time TARGET_SHOT_TIME
  ./coffee.sh graph --recipe RECIPE --time TARGET_SHOT_TIME [--output graph.svg]
  ./coffee.sh remove --recipe RECIPE"
    );
}

fn list_recipes(data_file: &Path) -> Result<(), Box<dyn Error>> {
    let data = load_data(data_file)?;

    if data.recipes.is_empty() {
        println!("No coffee recipes found.");
        return Ok(());
    }

    for recipe in data.recipes {
        println!("recipe: {}", recipe.name);
        println!("dose_weight_g: {}", recipe.dose_weight_g);
        if !recipe.samples.is_empty() {
            println!("samples:");
            for sample in recipe.samples {
                println!("  - time: {}", sample.time);
                println!("    grind: {}", sample.grind);
            }
        }
        println!("---");
    }

    Ok(())
}

fn add_recipe(data_file: &Path, args: &[String]) -> Result<(), Box<dyn Error>> {
    let mut parser = ArgParser::new(args);
    let mut recipe = None;
    let mut dose = None;

    while let Some(arg) = parser.next() {
        match arg {
            "--recipe" | "--name" => recipe = Some(parser.require_value(arg)?.to_string()),
            "--dose" => dose = Some(numeric_dose(parser.require_value(arg)?)),
            "-h" | "--help" => {
                print_usage();
                return Ok(());
            }
            _ => {
                print_usage_to_stderr();
                return Err(Box::new(AppError::new(format!(
                    "Unknown option for add: {arg}"
                ))));
            }
        }
    }

    let recipe = recipe.ok_or_else(|| {
        print_usage_to_stderr();
        AppError::new("Add requires --recipe and --dose")
    })?;
    let dose = dose.ok_or_else(|| {
        print_usage_to_stderr();
        AppError::new("Add requires --recipe and --dose")
    })?;

    reject_tabs("recipe", &recipe)?;
    reject_tabs("dose", &dose)?;
    require_number("dose", &dose)?;

    let mut data = load_data(data_file)?;
    if data.recipes.iter().any(|item| item.name == recipe) {
        return Err(Box::new(AppError::new(format!(
            "Recipe already exists: {recipe}"
        ))));
    }

    data.recipes.push(Recipe {
        name: recipe.clone(),
        dose_weight_g: dose,
        samples: Vec::new(),
    });
    save_data(data_file, &data)?;
    println!("Added recipe: {recipe}");
    Ok(())
}

fn add_sample(data_file: &Path, args: &[String]) -> Result<(), Box<dyn Error>> {
    let mut parser = ArgParser::new(args);
    let mut recipe = None;
    let mut grind = None;
    let mut shot_time = None;

    while let Some(arg) = parser.next() {
        match arg {
            "--recipe" | "--name" => recipe = Some(parser.require_value(arg)?.to_string()),
            "--grind" => grind = Some(numeric_plain(parser.require_value(arg)?)),
            "--time" => shot_time = Some(numeric_time(parser.require_value(arg)?)),
            "-h" | "--help" => {
                print_usage();
                return Ok(());
            }
            _ => {
                print_usage_to_stderr();
                return Err(Box::new(AppError::new(format!(
                    "Unknown option for sample: {arg}"
                ))));
            }
        }
    }

    let recipe = recipe.ok_or_else(|| {
        print_usage_to_stderr();
        AppError::new("Sample requires --recipe, --grind, and --time")
    })?;
    let grind = grind.ok_or_else(|| {
        print_usage_to_stderr();
        AppError::new("Sample requires --recipe, --grind, and --time")
    })?;
    let shot_time = shot_time.ok_or_else(|| {
        print_usage_to_stderr();
        AppError::new("Sample requires --recipe, --grind, and --time")
    })?;

    reject_tabs("recipe", &recipe)?;
    reject_tabs("grind", &grind)?;
    reject_tabs("shot time", &shot_time)?;
    require_grind_setting(&grind)?;
    require_number("shot time", &shot_time)?;

    let mut data = load_data(data_file)?;
    let existing = data
        .recipes
        .iter_mut()
        .find(|item| item.name == recipe)
        .ok_or_else(|| AppError::new(format!("Recipe not found: {recipe}")))?;

    existing.samples.push(Sample {
        recipe: recipe.clone(),
        time: format!("{shot_time}s"),
        grind,
    });
    save_data(data_file, &data)?;
    println!("Added sample for recipe: {recipe}");
    Ok(())
}

fn predict_recipe(data_file: &Path, args: &[String]) -> Result<(), Box<dyn Error>> {
    let mut parser = ArgParser::new(args);
    let mut recipe_name = None;
    let mut target_time = None;

    while let Some(arg) = parser.next() {
        match arg {
            "--recipe" | "--name" => recipe_name = Some(parser.require_value(arg)?.to_string()),
            "--time" => target_time = Some(parser.require_value(arg)?.to_string()),
            "-h" | "--help" => {
                print_usage();
                return Ok(());
            }
            _ => {
                print_usage_to_stderr();
                return Err(Box::new(AppError::new(format!(
                    "Unknown option for predict: {arg}"
                ))));
            }
        }
    }

    let recipe_name = recipe_name.ok_or_else(|| {
        print_usage_to_stderr();
        AppError::new("Predict requires --recipe and --time")
    })?;
    let target_time = target_time.ok_or_else(|| {
        print_usage_to_stderr();
        AppError::new("Predict requires --recipe and --time")
    })?;

    reject_tabs("recipe", &recipe_name)?;
    reject_tabs("target time", &target_time)?;
    let target_seconds_text = numeric_value(&target_time);
    require_number_with_message(
        &target_seconds_text,
        "Target time must be numeric, optionally ending in s",
    )?;
    let target_seconds = parse_number(&target_seconds_text);

    let data = load_data(data_file)?;
    let recipe = data
        .recipes
        .iter()
        .find(|item| item.name == recipe_name)
        .ok_or_else(|| AppError::new(format!("Recipe not found: {recipe_name}")))?;

    if recipe.samples.is_empty() {
        return Err(Box::new(AppError::new(format!(
            "No samples found for recipe: {recipe_name}"
        ))));
    }

    let mut grind_points = Vec::new();
    let mut non_numeric_grind = 0usize;
    let mut nearest: Option<(&Sample, f64)> = None;

    for sample in &recipe.samples {
        let shot_time = numeric_value(&sample.time);
        if !is_number(&shot_time) {
            continue;
        }
        let shot_time = parse_number(&shot_time);
        let grind = numeric_value(&sample.grind);
        if is_number(&grind) {
            grind_points.push((parse_number(&grind), shot_time));
        } else if !sample.grind.is_empty() {
            non_numeric_grind += 1;
        }

        let distance = (shot_time - target_seconds).abs();
        if nearest.is_none_or(|(_, nearest_distance)| distance < nearest_distance) {
            nearest = Some((sample, distance));
        }
    }

    println!("recipe: {recipe_name}");
    println!("target_time: {}s", fmt(target_seconds));
    println!("dose_weight_g: {}", recipe.dose_weight_g);
    println!("samples_used: {}", recipe.samples.len());

    let model_points = local_model_points(&grind_points, target_seconds, LOCAL_MODEL_SAMPLE_LIMIT);
    if !model_points.is_empty() {
        println!("model_samples_used: {}", model_points.len());
    }

    let predictions = report_prediction("grind", "", &model_points, target_seconds);

    if non_numeric_grind > 0 && grind_points.len() < 2 {
        println!("grind_note: grind regression requires at least two numeric grind settings");
    }

    if let Some((sample, _)) = nearest {
        println!("nearest_sample:");
        println!("  time: {}", sample.time);
        println!("  grind: {}", sample.grind);
    }

    if predictions == 0 {
        println!(
            "prediction_note: add at least two samples with varying numeric grind values to enable regression"
        );
    }

    Ok(())
}

fn graph_recipe(data_file: &Path, args: &[String]) -> Result<(), Box<dyn Error>> {
    let mut parser = ArgParser::new(args);
    let mut recipe_name = None;
    let mut target_time = None;
    let mut output = PathBuf::from("coffee_graph.svg");

    while let Some(arg) = parser.next() {
        match arg {
            "--recipe" | "--name" => recipe_name = Some(parser.require_value(arg)?.to_string()),
            "--time" => target_time = Some(parser.require_value(arg)?.to_string()),
            "--output" | "-o" => output = PathBuf::from(parser.require_value(arg)?),
            "-h" | "--help" => {
                print_usage();
                return Ok(());
            }
            _ => {
                print_usage_to_stderr();
                return Err(Box::new(AppError::new(format!(
                    "Unknown option for graph: {arg}"
                ))));
            }
        }
    }

    let recipe_name = recipe_name.ok_or_else(|| {
        print_usage_to_stderr();
        AppError::new("Graph requires --recipe and --time")
    })?;
    let target_time = target_time.ok_or_else(|| {
        print_usage_to_stderr();
        AppError::new("Graph requires --recipe and --time")
    })?;

    reject_tabs("recipe", &recipe_name)?;
    reject_tabs("target time", &target_time)?;
    let target_seconds_text = numeric_value(&target_time);
    require_number_with_message(
        &target_seconds_text,
        "Target time must be numeric, optionally ending in s",
    )?;
    let target_seconds = parse_number(&target_seconds_text);

    let data = load_data(data_file)?;
    let recipe = data
        .recipes
        .iter()
        .find(|item| item.name == recipe_name)
        .ok_or_else(|| AppError::new(format!("Recipe not found: {recipe_name}")))?;

    let points = numeric_grind_points(recipe);
    let model_points = local_model_points(&points, target_seconds, LOCAL_MODEL_SAMPLE_LIMIT);
    let (intercept, slope) = theil_sen_model(&model_points).ok_or_else(|| {
        AppError::new("Graph requires at least two samples with varying numeric grind values")
    })?;
    let predicted_grind = (target_seconds - intercept) / slope;
    let model_r2 = r_squared(&model_points, intercept, slope);
    let svg = render_graph_svg(
        recipe,
        &points,
        &model_points,
        target_seconds,
        predicted_grind,
        intercept,
        slope,
        model_r2,
    );

    fs::write(&output, svg)?;
    println!("Wrote graph: {}", output.display());
    Ok(())
}

fn remove_recipe(data_file: &Path, args: &[String]) -> Result<(), Box<dyn Error>> {
    let mut parser = ArgParser::new(args);
    let mut recipe = None;

    while let Some(arg) = parser.next() {
        match arg {
            "--recipe" | "--name" => recipe = Some(parser.require_value(arg)?.to_string()),
            "-h" | "--help" => {
                print_usage();
                return Ok(());
            }
            _ => {
                print_usage_to_stderr();
                return Err(Box::new(AppError::new(format!(
                    "Unknown option for remove: {arg}"
                ))));
            }
        }
    }

    let recipe = recipe.ok_or_else(|| {
        print_usage_to_stderr();
        AppError::new("Remove requires --recipe")
    })?;

    let mut data = load_data(data_file)?;
    let before = data.recipes.len();
    data.recipes.retain(|item| item.name != recipe);
    if data.recipes.len() == before {
        return Err(Box::new(AppError::new(format!(
            "Recipe not found: {recipe}"
        ))));
    }

    save_data(data_file, &data)?;
    println!("Removed recipe: {recipe}");
    Ok(())
}

fn load_data(data_file: &Path) -> Result<Data, Box<dyn Error>> {
    ensure_data_file(data_file)?;
    let content = fs::read_to_string(data_file)?;
    let mut lines = content.lines();
    let header = lines.next().unwrap_or("");

    if header == "name\tdose_size\tgrind\tshot_time\tbean" {
        let data = parse_legacy_recipe_data(lines);
        save_data(data_file, &data)?;
        return Ok(data);
    }

    if header == "record_type\trecipe\ttime\tgrind\tdose_weight_g" {
        let data = parse_sample_dose_data(lines);
        save_data(data_file, &data)?;
        return Ok(data);
    }

    let mut data = Data::default();
    for line in lines {
        let fields: Vec<&str> = line.split('\t').collect();
        match fields.first().copied() {
            Some("recipe") => data.recipes.push(Recipe {
                name: get_field(&fields, 1).to_string(),
                dose_weight_g: get_field(&fields, 2).to_string(),
                samples: Vec::new(),
            }),
            Some("sample") => {
                let sample = Sample {
                    recipe: get_field(&fields, 1).to_string(),
                    time: get_field(&fields, 3).to_string(),
                    grind: get_field(&fields, 4).to_string(),
                };
                if let Some(recipe) = data
                    .recipes
                    .iter_mut()
                    .find(|item| item.name == sample.recipe)
                {
                    recipe.samples.push(sample);
                }
            }
            _ => {}
        }
    }

    Ok(data)
}

fn parse_legacy_recipe_data<'a>(lines: impl Iterator<Item = &'a str>) -> Data {
    let mut data = Data::default();
    for line in lines {
        let fields: Vec<&str> = line.split('\t').collect();
        let name = get_field(&fields, 0).to_string();
        let dose = numeric_value(get_field(&fields, 1));
        let grind = get_field(&fields, 2);
        let time = get_field(&fields, 3);

        let mut recipe = Recipe {
            name: name.clone(),
            dose_weight_g: dose,
            samples: Vec::new(),
        };

        if !time.is_empty()
            && time.to_ascii_lowercase() != "unknown"
            && is_number(&numeric_value(grind))
        {
            recipe.samples.push(Sample {
                recipe: name,
                time: time.to_string(),
                grind: grind.to_string(),
            });
        }
        data.recipes.push(recipe);
    }
    data
}

fn parse_sample_dose_data<'a>(lines: impl Iterator<Item = &'a str>) -> Data {
    let mut data = Data::default();
    let mut deferred_samples: Vec<Sample> = Vec::new();

    for line in lines {
        let fields: Vec<&str> = line.split('\t').collect();
        match fields.first().copied() {
            Some("recipe") => data.recipes.push(Recipe {
                name: get_field(&fields, 1).to_string(),
                dose_weight_g: String::new(),
                samples: Vec::new(),
            }),
            Some("sample") => {
                let recipe_name = get_field(&fields, 1).to_string();
                let dose = numeric_value(get_field(&fields, 4));
                if let Some(recipe) = data
                    .recipes
                    .iter_mut()
                    .find(|item| item.name == recipe_name)
                {
                    if recipe.dose_weight_g.is_empty() && !dose.is_empty() {
                        recipe.dose_weight_g = dose;
                    }
                }
                if is_number(&numeric_value(get_field(&fields, 3))) {
                    deferred_samples.push(Sample {
                        recipe: recipe_name,
                        time: get_field(&fields, 2).to_string(),
                        grind: numeric_value(get_field(&fields, 3)),
                    });
                }
            }
            _ => {}
        }
    }

    for sample in deferred_samples {
        if let Some(recipe) = data
            .recipes
            .iter_mut()
            .find(|item| item.name == sample.recipe)
        {
            recipe.samples.push(sample);
        }
    }

    data
}

fn ensure_data_file(data_file: &Path) -> io::Result<()> {
    if !data_file.exists() {
        fs::write(data_file, HEADER)?;
    }
    Ok(())
}

fn save_data(data_file: &Path, data: &Data) -> io::Result<()> {
    let mut output = String::from(HEADER);
    for recipe in &data.recipes {
        output.push_str("recipe\t");
        output.push_str(&recipe.name);
        output.push('\t');
        output.push_str(&recipe.dose_weight_g);
        output.push_str("\t\t\n");
        for sample in &recipe.samples {
            output.push_str("sample\t");
            output.push_str(&sample.recipe);
            output.push_str("\t\t");
            output.push_str(&sample.time);
            output.push('\t');
            output.push_str(&sample.grind);
            output.push('\n');
        }
    }
    fs::write(data_file, output)
}

fn get_field<'a>(fields: &'a [&str], index: usize) -> &'a str {
    fields.get(index).copied().unwrap_or("")
}

fn reject_tabs(field: &str, value: &str) -> Result<(), Box<dyn Error>> {
    if value.contains('\t') || value.contains('\n') {
        return Err(Box::new(AppError::new(format!(
            "{field} cannot contain tabs or newlines"
        ))));
    }
    Ok(())
}

fn numeric_value(value: &str) -> String {
    let mut cleaned = remove_whitespace(value);
    if cleaned.ends_with('g') || cleaned.ends_with('s') {
        cleaned.pop();
    }
    cleaned
}

fn numeric_dose(value: &str) -> String {
    let mut cleaned = remove_whitespace(value);
    if cleaned.ends_with('g') {
        cleaned.pop();
    }
    cleaned
}

fn numeric_time(value: &str) -> String {
    let mut cleaned = remove_whitespace(value);
    if cleaned.ends_with('s') {
        cleaned.pop();
    }
    cleaned
}

fn numeric_plain(value: &str) -> String {
    remove_whitespace(value)
}

fn remove_whitespace(value: &str) -> String {
    value
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect()
}

fn require_grind_setting(value: &str) -> Result<(), Box<dyn Error>> {
    require_number("grind", value)?;
    let grind = parse_number(value);
    if !(1.0..=40.0).contains(&grind) {
        return Err(Box::new(AppError::new("grind must be between 1 and 40")));
    }
    Ok(())
}

fn require_number(field: &str, value: &str) -> Result<(), Box<dyn Error>> {
    if !is_number(value) {
        return Err(Box::new(AppError::new(format!("{field} must be numeric"))));
    }
    Ok(())
}

fn require_number_with_message(value: &str, message: &str) -> Result<(), Box<dyn Error>> {
    if !is_number(value) {
        return Err(Box::new(AppError::new(message)));
    }
    Ok(())
}

fn is_number(value: &str) -> bool {
    if value.is_empty() {
        return false;
    }
    let bytes = value.as_bytes();
    let mut index = usize::from(bytes[0] == b'-');
    if index == bytes.len() {
        return false;
    }

    let mut digits = 0usize;
    while index < bytes.len() && bytes[index].is_ascii_digit() {
        digits += 1;
        index += 1;
    }

    if index < bytes.len() && bytes[index] == b'.' {
        index += 1;
        while index < bytes.len() && bytes[index].is_ascii_digit() {
            digits += 1;
            index += 1;
        }
    }

    digits > 0 && index == bytes.len()
}

fn parse_number(value: &str) -> f64 {
    value.parse::<f64>().expect("validated numeric value")
}

fn fmt(value: f64) -> String {
    format!("{value:.2}")
}

fn numeric_grind_points(recipe: &Recipe) -> Vec<(f64, f64)> {
    recipe
        .samples
        .iter()
        .filter_map(|sample| {
            let grind = numeric_value(&sample.grind);
            let shot_time = numeric_value(&sample.time);
            if is_number(&grind) && is_number(&shot_time) {
                Some((parse_number(&grind), parse_number(&shot_time)))
            } else {
                None
            }
        })
        .collect()
}

fn local_model_points(
    points: &[(f64, f64)],
    target_seconds: f64,
    sample_limit: usize,
) -> Vec<(f64, f64)> {
    let mut local = points.to_vec();
    local.sort_by(|left, right| {
        let left_distance = (left.1 - target_seconds).abs();
        let right_distance = (right.1 - target_seconds).abs();
        left_distance
            .total_cmp(&right_distance)
            .then_with(|| left.1.total_cmp(&right.1))
            .then_with(|| left.0.total_cmp(&right.0))
    });
    local.truncate(sample_limit);
    local
}

fn grind_axis_bounds(points: &[(f64, f64)]) -> (f64, f64) {
    let mut min = points
        .iter()
        .map(|(grind, _)| *grind)
        .fold(f64::INFINITY, f64::min)
        .floor();
    let mut max = points
        .iter()
        .map(|(grind, _)| *grind)
        .fold(f64::NEG_INFINITY, f64::max)
        .ceil();

    if !min.is_finite() || !max.is_finite() {
        return (1.0, 40.0);
    }

    if min == max {
        min -= 1.0;
        max += 1.0;
    }

    (min, max)
}

fn axis_ticks(min: f64, max: f64) -> Vec<f64> {
    if min == max {
        return vec![min];
    }

    let steps = 4.0;
    (0..=4)
        .map(|index| min + ((max - min) * index as f64 / steps))
        .collect()
}

fn axis_label(value: f64) -> String {
    if (value.round() - value).abs() < 0.001 {
        format!("{value:.0}")
    } else {
        format!("{value:.1}")
    }
}

fn render_graph_svg(
    recipe: &Recipe,
    points: &[(f64, f64)],
    model_points: &[(f64, f64)],
    target_seconds: f64,
    predicted_grind: f64,
    intercept: f64,
    slope: f64,
    model_r2: f64,
) -> String {
    let width = 960.0;
    let height = 560.0;
    let left = 84.0;
    let right = 34.0;
    let top = 74.0;
    let bottom = 74.0;
    let plot_width = width - left - right;
    let plot_height = height - top - bottom;
    let (x_min, x_max) = grind_axis_bounds(points);
    let y_min = 0.0;
    let y_max = 60.0;
    let model_y_at_min = intercept + (slope * x_min);
    let model_y_at_max = intercept + (slope * x_max);

    let x = |grind: f64| left + ((grind - x_min) / (x_max - x_min) * plot_width);
    let y = |seconds: f64| top + ((y_max - seconds) / (y_max - y_min) * plot_height);

    let line_x1 = x(x_min);
    let line_y1 = y(model_y_at_min);
    let line_x2 = x(x_max);
    let line_y2 = y(model_y_at_max);
    let target_y = y(target_seconds);
    let predicted_x = x(predicted_grind);
    let title = svg_escape(&recipe.name);
    let subtitle = svg_escape(&format!(
        "dose {}g - {} samples - local model {} samples - time = {} + {} * grind - R2 {}",
        recipe.dose_weight_g,
        points.len(),
        model_points.len(),
        fmt(intercept),
        fmt(slope),
        fmt(model_r2)
    ));

    let mut svg = String::new();
    svg.push_str(&format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{width:.0}" height="{height:.0}" viewBox="0 0 {width:.0} {height:.0}" role="img" aria-labelledby="title desc">
<title id="title">Shot time vs grind for {title}</title>
<desc id="desc">Coffee recipe graph showing recorded shot samples and Theil-Sen prediction line.</desc>
<rect width="100%" height="100%" fill="#111827"/>
<text x="{left:.0}" y="34" font-family="Arial, sans-serif" font-size="22" font-weight="700" fill="#f8fafc">{title}</text>
<text x="{left:.0}" y="58" font-family="Arial, sans-serif" font-size="13" fill="#94a3b8">{subtitle}</text>
<rect x="{left:.0}" y="{top:.0}" width="{plot_width:.0}" height="{plot_height:.0}" fill="#172033" stroke="#334155"/>
<clipPath id="plot-area"><rect x="{left:.0}" y="{top:.0}" width="{plot_width:.0}" height="{plot_height:.0}"/></clipPath>
"##
    ));

    for tick in axis_ticks(x_min, x_max) {
        let tick_x = x(tick);
        svg.push_str(&format!(
            r##"<line x1="{tick_x:.2}" y1="{top:.2}" x2="{tick_x:.2}" y2="{:.2}" stroke="#243044"/>
<line x1="{tick_x:.2}" y1="{:.2}" x2="{tick_x:.2}" y2="{:.2}" stroke="#64748b"/>
<text x="{tick_x:.2}" y="{:.2}" text-anchor="middle" font-family="Arial, sans-serif" font-size="12" fill="#cbd5e1">{}</text>
"##,
            top + plot_height,
            top + plot_height,
            top + plot_height + 6.0,
            top + plot_height + 24.0,
            axis_label(tick)
        ));
    }

    for index in 0..=4 {
        let tick = y_min + ((y_max - y_min) * index as f64 / 4.0);
        let tick_y = y(tick);
        svg.push_str(&format!(
            r##"<line x1="{left:.2}" y1="{tick_y:.2}" x2="{:.2}" y2="{tick_y:.2}" stroke="#243044"/>
<line x1="{:.2}" y1="{tick_y:.2}" x2="{left:.2}" y2="{tick_y:.2}" stroke="#64748b"/>
<text x="{:.2}" y="{:.2}" text-anchor="end" font-family="Arial, sans-serif" font-size="12" fill="#cbd5e1">{}s</text>
"##,
            left + plot_width,
            left - 6.0,
            left - 12.0,
            tick_y + 4.0,
            fmt(tick)
        ));
    }

    svg.push_str(&format!(
        r##"<g clip-path="url(#plot-area)">
<line x1="{line_x1:.2}" y1="{line_y1:.2}" x2="{line_x2:.2}" y2="{line_y2:.2}" stroke="#60a5fa" stroke-width="3"/>
<line x1="{left:.2}" y1="{target_y:.2}" x2="{:.2}" y2="{target_y:.2}" stroke="#f59e0b" stroke-width="2" stroke-dasharray="7 5"/>
"##,
        left + plot_width
    ));

    if (x_min..=x_max).contains(&predicted_grind) {
        svg.push_str(&format!(
            r##"<line x1="{predicted_x:.2}" y1="{top:.2}" x2="{predicted_x:.2}" y2="{:.2}" stroke="#f59e0b" stroke-width="2" stroke-dasharray="7 5"/>
<circle cx="{predicted_x:.2}" cy="{target_y:.2}" r="6" fill="#f59e0b" stroke="#111827" stroke-width="2"/>
"##,
            top + plot_height
        ));
    }

    for (grind, seconds) in points {
        let is_model_point = model_points
            .iter()
            .any(|(model_grind, model_seconds)| model_grind == grind && model_seconds == seconds);
        let (fill, radius) = if is_model_point {
            ("#2dd4bf", 5.0)
        } else {
            ("#64748b", 4.0)
        };
        svg.push_str(&format!(
            r##"<circle cx="{:.2}" cy="{:.2}" r="{radius}" fill="{fill}" stroke="#0f172a" stroke-width="2"/>
"##,
            x(*grind),
            y(*seconds)
        ));
    }
    svg.push_str("</g>\n");

    svg.push_str(&format!(
        r##"<text x="{:.2}" y="{:.2}" font-family="Arial, sans-serif" font-size="13" fill="#e2e8f0">grind setting</text>
<text x="22" y="{:.2}" transform="rotate(-90 22 {:.2})" font-family="Arial, sans-serif" font-size="13" fill="#e2e8f0">shot time seconds</text>
<g font-family="Arial, sans-serif" font-size="13" fill="#e2e8f0">
  <rect x="{:.2}" y="90" width="286" height="89" rx="6" fill="#0f172a" stroke="#334155"/>
  <circle cx="{:.2}" cy="111" r="5" fill="#2dd4bf"/>
  <text x="{:.2}" y="116">local model sample</text>
  <circle cx="{:.2}" cy="136" r="4" fill="#64748b"/>
  <text x="{:.2}" y="141">context sample</text>
  <line x1="{:.2}" y1="159" x2="{:.2}" y2="159" stroke="#60a5fa" stroke-width="3"/>
  <text x="{:.2}" y="164">local Theil-Sen line</text>
  <text x="{:.2}" y="199" font-size="14" font-weight="700" fill="#fbbf24">target {}s -> grind {}</text>
</g>
</svg>
"##,
        left + (plot_width / 2.0) - 34.0,
        height - 18.0,
        top + (plot_height / 2.0) + 56.0,
        top + (plot_height / 2.0) + 56.0,
        width - 330.0,
        width - 310.0,
        width - 294.0,
        width - 310.0,
        width - 294.0,
        width - 316.0,
        width - 286.0,
        width - 274.0,
        width - 330.0,
        fmt(target_seconds),
        fmt(predicted_grind)
    ));

    svg
}

fn svg_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn report_prediction(
    label: &str,
    suffix: &str,
    points: &[(f64, f64)],
    target_seconds: f64,
) -> usize {
    let Some((intercept, slope)) = theil_sen_model(points) else {
        return 0;
    };

    let predicted = (target_seconds - intercept) / slope;
    let r_squared = r_squared(points, intercept, slope);

    println!("{label}: {}{suffix}", fmt(predicted));
    println!(
        "{label}_model: time = {} + {} * {label}",
        fmt(intercept),
        fmt(slope)
    );
    println!("{label}_r_squared: {}", fmt(r_squared));
    1
}

fn theil_sen_model(points: &[(f64, f64)]) -> Option<(f64, f64)> {
    if points.len() < 2 {
        return None;
    }

    let mut slopes = Vec::new();
    for (index, (x1, y1)) in points.iter().enumerate() {
        for (x2, y2) in &points[index + 1..] {
            let dx = x2 - x1;
            if dx != 0.0 {
                slopes.push((y2 - y1) / dx);
            }
        }
    }

    let slope = median(slopes)?;
    if slope == 0.0 {
        return None;
    }

    let intercepts = points
        .iter()
        .map(|(x, y)| y - (slope * x))
        .collect::<Vec<_>>();
    let intercept = median(intercepts)?;

    Some((intercept, slope))
}

fn median(mut values: Vec<f64>) -> Option<f64> {
    if values.is_empty() {
        return None;
    }

    values.sort_by(|left, right| left.total_cmp(right));
    let middle = values.len() / 2;
    if values.len() % 2 == 0 {
        Some((values[middle - 1] + values[middle]) / 2.0)
    } else {
        Some(values[middle])
    }
}

fn r_squared(points: &[(f64, f64)], intercept: f64, slope: f64) -> f64 {
    if points.is_empty() {
        return 1.0;
    }

    let mean_y = points.iter().map(|(_, y)| y).sum::<f64>() / points.len() as f64;
    let ss_tot = points
        .iter()
        .map(|(_, y)| {
            let error = y - mean_y;
            error * error
        })
        .sum::<f64>();
    let ss_res = points
        .iter()
        .map(|(x, y)| {
            let error = y - (intercept + (slope * x));
            error * error
        })
        .sum::<f64>();

    if ss_tot == 0.0 {
        1.0
    } else {
        1.0 - (ss_res / ss_tot)
    }
}

struct ArgParser<'a> {
    args: &'a [String],
    index: usize,
}

impl<'a> ArgParser<'a> {
    fn new(args: &'a [String]) -> Self {
        Self { args, index: 0 }
    }

    fn next(&mut self) -> Option<&'a str> {
        let value = self.args.get(self.index)?;
        self.index += 1;
        Some(value)
    }

    fn require_value(&mut self, option: &str) -> Result<&'a str, Box<dyn Error>> {
        let value = self
            .args
            .get(self.index)
            .ok_or_else(|| AppError::new(format!("Missing value for {option}")))?;
        if value.is_empty() || value.starts_with("--") {
            return Err(Box::new(AppError::new(format!(
                "Missing value for {option}"
            ))));
        }
        self.index += 1;
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theil_sen_matches_two_point_line() {
        let points = vec![(16.0, 25.0), (20.0, 35.0)];

        let (intercept, slope) = theil_sen_model(&points).expect("model");

        assert_eq!(fmt(intercept), "-15.00");
        assert_eq!(fmt(slope), "2.50");
        assert_eq!(fmt((30.0 - intercept) / slope), "18.00");
    }

    #[test]
    fn theil_sen_resists_single_outlier() {
        let points = vec![
            (14.0, 20.0),
            (16.0, 25.0),
            (18.0, 30.0),
            (20.0, 35.0),
            (22.0, 40.0),
            (40.0, 5.0),
        ];

        let (intercept, slope) = theil_sen_model(&points).expect("model");

        assert_eq!(fmt((30.0 - intercept) / slope), "18.00");
    }

    #[test]
    fn local_model_points_keep_closest_shot_times() {
        let points = vec![
            (12.0, 52.0),
            (13.0, 43.0),
            (14.0, 36.0),
            (15.0, 31.0),
            (16.0, 28.0),
            (17.0, 25.5),
            (18.0, 24.0),
            (19.0, 23.0),
        ];

        let local = local_model_points(&points, 30.0, 6);

        assert_eq!(local.len(), 6);
        assert!(!local.contains(&(12.0, 52.0)));
        assert!(!local.contains(&(13.0, 43.0)));
        assert!(local.contains(&(15.0, 31.0)));
        assert!(local.contains(&(16.0, 28.0)));
    }

    #[test]
    fn grind_axis_matches_sample_extent() {
        let points = vec![(12.2, 40.0), (18.7, 25.0)];

        assert_eq!(grind_axis_bounds(&points), (12.0, 19.0));
    }

    #[test]
    fn graph_svg_contains_prediction_context() {
        let recipe = Recipe {
            name: "Test & Espresso".to_string(),
            dose_weight_g: "18".to_string(),
            samples: Vec::new(),
        };
        let points = vec![(16.0, 25.0), (20.0, 35.0)];
        let model_points = points.clone();

        let svg = render_graph_svg(&recipe, &points, &model_points, 30.0, 18.0, -15.0, 2.5, 1.0);

        assert!(svg.contains("<svg"));
        assert!(svg.contains("Test &amp; Espresso"));
        assert!(svg.contains("target 30.00s -> grind 18.00"));
        assert!(svg.contains("local Theil-Sen line"));
        assert!(svg.contains(">60.00s</text>"));
    }
}
