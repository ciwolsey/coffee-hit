use std::env;
use std::error::Error;
use std::fmt::{self, Display};
use std::fs;
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process;

const HEADER: &str = "record_type\trecipe\tdose_weight_g\tshot_weight_g\ttime\tgrind\n";
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
    shot_weight_g: String,
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
        "serve" | "web" => serve_web(&data_file, &args),
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
  ./coffee.sh add --recipe RECIPE --dose DOSE_WEIGHT_G --shot-weight SHOT_WEIGHT_G
  ./coffee.sh sample --recipe RECIPE --time SHOT_TIME --grind GRIND
  ./coffee.sh sample --recipe RECIPE --grind GRIND --choked
  ./coffee.sh predict --recipe RECIPE --time TARGET_SHOT_TIME
  ./coffee.sh graph --recipe RECIPE --time TARGET_SHOT_TIME [--output graph.svg]
  ./coffee.sh serve [--host HOST] [--port 9000]
  ./coffee.sh remove --recipe RECIPE

Recipes are stored in coffee_recipes.tsv as:
  record_type<TAB>recipe<TAB>dose_weight_g<TAB>shot_weight_g<TAB>time<TAB>grind

Rows with record_type \"recipe\" define recipes and their fixed dose and shot
weight in grams.
Rows with record_type \"sample\" define shot samples for a recipe. A shot time
of 0s marks a choked shot and is excluded from regression. Grind is a
numeric grinder setting from 1 (finest) to 40 (very coarse)."
    );
}

fn print_usage_to_stderr() {
    eprintln!(
        "Usage:
  ./coffee.sh recipes
  ./coffee.sh add --recipe RECIPE --dose DOSE_WEIGHT_G --shot-weight SHOT_WEIGHT_G
  ./coffee.sh sample --recipe RECIPE --time SHOT_TIME --grind GRIND
  ./coffee.sh sample --recipe RECIPE --grind GRIND --choked
  ./coffee.sh predict --recipe RECIPE --time TARGET_SHOT_TIME
  ./coffee.sh graph --recipe RECIPE --time TARGET_SHOT_TIME [--output graph.svg]
  ./coffee.sh serve [--host HOST] [--port 9000]
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
        println!("shot_weight_g: {}", recipe.shot_weight_g);
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
    let mut shot_weight = None;

    while let Some(arg) = parser.next() {
        match arg {
            "--recipe" | "--name" => recipe = Some(parser.require_value(arg)?.to_string()),
            "--dose" => dose = Some(numeric_dose(parser.require_value(arg)?)),
            "--shot-weight" | "--yield" => {
                shot_weight = Some(numeric_dose(parser.require_value(arg)?))
            }
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
        AppError::new("Add requires --recipe, --dose, and --shot-weight")
    })?;
    let dose = dose.ok_or_else(|| {
        print_usage_to_stderr();
        AppError::new("Add requires --recipe, --dose, and --shot-weight")
    })?;
    let shot_weight = shot_weight.ok_or_else(|| {
        print_usage_to_stderr();
        AppError::new("Add requires --recipe, --dose, and --shot-weight")
    })?;

    reject_tabs("recipe", &recipe)?;
    reject_tabs("dose", &dose)?;
    reject_tabs("shot weight", &shot_weight)?;
    require_number("dose", &dose)?;
    require_number("shot weight", &shot_weight)?;

    let mut data = load_data(data_file)?;
    if data.recipes.iter().any(|item| item.name == recipe) {
        return Err(Box::new(AppError::new(format!(
            "Recipe already exists: {recipe}"
        ))));
    }

    data.recipes.push(Recipe {
        name: recipe.clone(),
        dose_weight_g: dose,
        shot_weight_g: shot_weight,
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
    let mut choked = false;

    while let Some(arg) = parser.next() {
        match arg {
            "--recipe" | "--name" => recipe = Some(parser.require_value(arg)?.to_string()),
            "--grind" => grind = Some(numeric_plain(parser.require_value(arg)?)),
            "--time" => shot_time = Some(numeric_time(parser.require_value(arg)?)),
            "--choked" => choked = true,
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
    if choked && shot_time.is_some() {
        print_usage_to_stderr();
        return Err(Box::new(AppError::new(
            "Choked samples record time as 0s and do not accept --time",
        )));
    }

    let shot_time = if choked {
        "0".to_string()
    } else {
        shot_time.ok_or_else(|| {
            print_usage_to_stderr();
            AppError::new("Sample requires --recipe, --grind, and --time")
        })?
    };

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
        if shot_time == 0.0 {
            continue;
        }
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
    println!("shot_weight_g: {}", recipe.shot_weight_g);
    println!("samples_used: {}", recipe.samples.len());

    let model_points = local_model_points(&grind_points, target_seconds, LOCAL_MODEL_SAMPLE_LIMIT);
    if !model_points.is_empty() {
        println!("model_samples_used: {}", model_points.len());
    }

    let predictions = report_prediction("grind", "", &model_points, target_seconds);
    if let Some(curve_grind) = exponential_predicted_grind(&model_points, target_seconds) {
        println!("curve_grind: {}", fmt(curve_grind));
        if let Some((log_intercept, log_slope)) = exponential_time_model(&model_points) {
            println!(
                "curve_model: ln(time) = {} + {} * grind",
                fmt(log_intercept),
                fmt(log_slope)
            );
        }
    }

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
    let choke_grinds = choked_grinds(recipe);
    let (intercept, slope) = theil_sen_model(&model_points).ok_or_else(|| {
        AppError::new("Graph requires at least two samples with varying numeric grind values")
    })?;
    let predicted_grind = (target_seconds - intercept) / slope;
    let model_r2 = r_squared(&model_points, intercept, slope);
    let svg = render_graph_svg(
        recipe,
        &points,
        &model_points,
        &choke_grinds,
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

fn serve_web(data_file: &Path, args: &[String]) -> Result<(), Box<dyn Error>> {
    let mut parser = ArgParser::new(args);
    let mut host = "0.0.0.0".to_string();
    let mut port = "9000".to_string();

    while let Some(arg) = parser.next() {
        match arg {
            "--host" => host = parser.require_value(arg)?.to_string(),
            "--port" => port = parser.require_value(arg)?.to_string(),
            "-h" | "--help" => {
                print_usage();
                return Ok(());
            }
            _ => {
                print_usage_to_stderr();
                return Err(Box::new(AppError::new(format!(
                    "Unknown option for serve: {arg}"
                ))));
            }
        }
    }

    require_number("port", &port)?;
    let address = format!("{host}:{port}");
    let listener = TcpListener::bind(&address)?;
    println!("Coffee web app listening on http://{address}");
    println!("Open this server from your phone or tablet using this machine's LAN IP address.");

    for stream in listener.incoming() {
        match stream {
            Ok(mut stream) => {
                if let Err(error) = handle_http_request(&mut stream, data_file) {
                    let body = json_error(&error.to_string());
                    let _ = write_http_response(
                        &mut stream,
                        "500 Internal Server Error",
                        "application/json; charset=utf-8",
                        body.as_bytes(),
                    );
                }
            }
            Err(error) => eprintln!("Connection failed: {error}"),
        }
    }

    Ok(())
}

fn handle_http_request(stream: &mut TcpStream, data_file: &Path) -> Result<(), Box<dyn Error>> {
    let request = read_http_request(stream)?;
    let mut lines = request.split("\r\n");
    let request_line = lines
        .next()
        .ok_or_else(|| AppError::new("Invalid HTTP request"))?;
    let mut request_parts = request_line.split_whitespace();
    let method = request_parts.next().unwrap_or("");
    let target = request_parts.next().unwrap_or("/");
    let body = request
        .split_once("\r\n\r\n")
        .map(|(_, body)| body)
        .unwrap_or("");
    let (path, query) = target.split_once('?').unwrap_or((target, ""));

    match (method, path) {
        ("GET", "/") | ("GET", "/index.html") => write_http_response(
            stream,
            "200 OK",
            "text/html; charset=utf-8",
            web_app_html().as_bytes(),
        )?,
        ("GET", "/api/state") => {
            let params = parse_form_encoded(query);
            let recipe = form_value(&params, "recipe");
            let target_time = form_value(&params, "time")
                .filter(|value| !value.is_empty())
                .unwrap_or("30");
            let json = web_state_json(data_file, recipe, target_time)?;
            write_http_response(
                stream,
                "200 OK",
                "application/json; charset=utf-8",
                json.as_bytes(),
            )?;
        }
        ("POST", "/api/recipes") => {
            let params = parse_form_encoded(body);
            create_recipe_from_form(data_file, &params)?;
            let recipe = form_value(&params, "recipe");
            let json = web_state_json(data_file, recipe, "30")?;
            write_http_response(
                stream,
                "200 OK",
                "application/json; charset=utf-8",
                json.as_bytes(),
            )?;
        }
        ("POST", "/api/samples") => {
            let params = parse_form_encoded(body);
            add_sample_from_form(data_file, &params)?;
            let recipe = form_value(&params, "recipe");
            let target_time = form_value(&params, "target_time").unwrap_or("30");
            let json = web_state_json(data_file, recipe, target_time)?;
            write_http_response(
                stream,
                "200 OK",
                "application/json; charset=utf-8",
                json.as_bytes(),
            )?;
        }
        ("POST", "/api/samples/delete") => {
            let params = parse_form_encoded(body);
            delete_sample_from_form(data_file, &params)?;
            let recipe = form_value(&params, "recipe");
            let target_time = form_value(&params, "target_time").unwrap_or("30");
            let json = web_state_json(data_file, recipe, target_time)?;
            write_http_response(
                stream,
                "200 OK",
                "application/json; charset=utf-8",
                json.as_bytes(),
            )?;
        }
        ("POST", "/api/recipes/delete") => {
            let params = parse_form_encoded(body);
            delete_recipe_from_form(data_file, &params)?;
            let target_time = form_value(&params, "target_time").unwrap_or("30");
            let json = web_state_json(data_file, None, target_time)?;
            write_http_response(
                stream,
                "200 OK",
                "application/json; charset=utf-8",
                json.as_bytes(),
            )?;
        }
        _ => write_http_response(
            stream,
            "404 Not Found",
            "application/json; charset=utf-8",
            json_error("Not found").as_bytes(),
        )?,
    }

    Ok(())
}

fn read_http_request(stream: &mut TcpStream) -> io::Result<String> {
    let mut buffer = [0; 8192];
    let mut bytes = Vec::new();
    let mut header_end = None;

    loop {
        let read = stream.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..read]);
        if header_end.is_none() {
            header_end = find_bytes(&bytes, b"\r\n\r\n");
        }
        if let Some(end) = header_end {
            let headers = String::from_utf8_lossy(&bytes[..end]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    if name.eq_ignore_ascii_case("content-length") {
                        value.trim().parse::<usize>().ok()
                    } else {
                        None
                    }
                })
                .unwrap_or(0);
            if bytes.len() >= end + 4 + content_length {
                break;
            }
        }
        if bytes.len() > 1_000_000 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "HTTP request is too large",
            ));
        }
    }

    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn write_http_response(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &[u8],
) -> io::Result<()> {
    let headers = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(headers.as_bytes())?;
    stream.write_all(body)
}

fn create_recipe_from_form(
    data_file: &Path,
    params: &[(String, String)],
) -> Result<(), Box<dyn Error>> {
    let recipe = form_value(params, "recipe")
        .ok_or_else(|| AppError::new("Recipe name is required"))?
        .trim()
        .to_string();
    let dose = numeric_dose(
        form_value(params, "dose")
            .ok_or_else(|| AppError::new("Dose is required"))?
            .trim(),
    );
    let shot_weight = numeric_dose(
        form_value(params, "shot_weight")
            .ok_or_else(|| AppError::new("Shot weight is required"))?
            .trim(),
    );

    reject_tabs("recipe", &recipe)?;
    reject_tabs("dose", &dose)?;
    reject_tabs("shot weight", &shot_weight)?;
    require_number("dose", &dose)?;
    require_number("shot weight", &shot_weight)?;

    let mut data = load_data(data_file)?;
    if data.recipes.iter().any(|item| item.name == recipe) {
        return Err(Box::new(AppError::new(format!(
            "Recipe already exists: {recipe}"
        ))));
    }

    data.recipes.push(Recipe {
        name: recipe,
        dose_weight_g: dose,
        shot_weight_g: shot_weight,
        samples: Vec::new(),
    });
    save_data(data_file, &data)?;
    Ok(())
}

fn add_sample_from_form(
    data_file: &Path,
    params: &[(String, String)],
) -> Result<(), Box<dyn Error>> {
    let recipe = form_value(params, "recipe")
        .ok_or_else(|| AppError::new("Choose a recipe first"))?
        .trim()
        .to_string();
    let choked = form_value(params, "choked").is_some_and(|value| value == "1");
    let shot_time = if choked {
        "0".to_string()
    } else {
        numeric_time(
            form_value(params, "time")
                .ok_or_else(|| AppError::new("Shot time is required"))?
                .trim(),
        )
    };
    let grind = numeric_plain(
        form_value(params, "grind")
            .ok_or_else(|| AppError::new("Grind is required"))?
            .trim(),
    );

    reject_tabs("recipe", &recipe)?;
    reject_tabs("shot time", &shot_time)?;
    reject_tabs("grind", &grind)?;
    require_number("shot time", &shot_time)?;
    require_grind_setting(&grind)?;

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
    Ok(())
}

fn delete_sample_from_form(
    data_file: &Path,
    params: &[(String, String)],
) -> Result<(), Box<dyn Error>> {
    let recipe = form_value(params, "recipe")
        .ok_or_else(|| AppError::new("Recipe is required"))?
        .trim()
        .to_string();
    let sample_index = form_value(params, "sample_index")
        .ok_or_else(|| AppError::new("Sample index is required"))?
        .parse::<usize>()
        .map_err(|_| AppError::new("Sample index must be numeric"))?;

    let mut data = load_data(data_file)?;
    let existing = data
        .recipes
        .iter_mut()
        .find(|item| item.name == recipe)
        .ok_or_else(|| AppError::new(format!("Recipe not found: {recipe}")))?;

    if sample_index >= existing.samples.len() {
        return Err(Box::new(AppError::new("Sample not found")));
    }

    existing.samples.remove(sample_index);
    save_data(data_file, &data)?;
    Ok(())
}

fn delete_recipe_from_form(
    data_file: &Path,
    params: &[(String, String)],
) -> Result<(), Box<dyn Error>> {
    let recipe = form_value(params, "recipe")
        .ok_or_else(|| AppError::new("Recipe is required"))?
        .trim()
        .to_string();

    let mut data = load_data(data_file)?;
    let before = data.recipes.len();
    data.recipes.retain(|item| item.name != recipe);
    if data.recipes.len() == before {
        return Err(Box::new(AppError::new(format!(
            "Recipe not found: {recipe}"
        ))));
    }

    save_data(data_file, &data)?;
    Ok(())
}

fn web_state_json(
    data_file: &Path,
    selected_recipe: Option<&str>,
    target_time: &str,
) -> Result<String, Box<dyn Error>> {
    let data = load_data(data_file)?;
    let selected_name = selected_recipe
        .filter(|name| data.recipes.iter().any(|recipe| recipe.name == *name))
        .or_else(|| data.recipes.first().map(|recipe| recipe.name.as_str()));
    let target_seconds_text = numeric_value(target_time);
    let target_seconds = if is_number(&target_seconds_text) {
        parse_number(&target_seconds_text)
    } else {
        30.0
    };

    let mut json = String::new();
    json.push_str("{\"recipes\":[");
    for (index, recipe) in data.recipes.iter().enumerate() {
        if index > 0 {
            json.push(',');
        }
        json.push_str(&recipe_json(recipe));
    }
    json.push_str("],");
    json.push_str("\"selected_recipe\":");
    if let Some(name) = selected_name {
        json_string_into(&mut json, name);
    } else {
        json.push_str("null");
    }
    json.push_str(&format!(",\"target_time\":\"{}\"", fmt(target_seconds)));
    json.push_str(",\"prediction\":");

    if let Some(recipe) =
        selected_name.and_then(|name| data.recipes.iter().find(|candidate| candidate.name == name))
    {
        json.push_str(&prediction_json(recipe, target_seconds));
    } else {
        json.push_str("null");
    }

    json.push('}');
    Ok(json)
}

fn recipe_json(recipe: &Recipe) -> String {
    let mut json = String::new();
    json.push('{');
    json.push_str("\"name\":");
    json_string_into(&mut json, &recipe.name);
    json.push_str(",\"dose_weight_g\":");
    json_string_into(&mut json, &recipe.dose_weight_g);
    json.push_str(",\"shot_weight_g\":");
    json_string_into(&mut json, &recipe.shot_weight_g);
    json.push_str(",\"sample_count\":");
    json.push_str(&recipe.samples.len().to_string());
    json.push_str(",\"samples\":[");
    for (index, sample) in recipe.samples.iter().enumerate() {
        if index > 0 {
            json.push(',');
        }
        json.push('{');
        json.push_str("\"index\":");
        json.push_str(&index.to_string());
        json.push_str(",\"time\":");
        json_string_into(&mut json, &sample.time);
        json.push_str(",\"grind\":");
        json_string_into(&mut json, &sample.grind);
        json.push_str(",\"choked\":");
        json.push_str(if is_choked_sample(sample) {
            "true"
        } else {
            "false"
        });
        json.push('}');
    }
    json.push_str("]}");
    json
}

fn prediction_json(recipe: &Recipe, target_seconds: f64) -> String {
    let points = numeric_grind_points(recipe);
    let model_points = local_model_points(&points, target_seconds, LOCAL_MODEL_SAMPLE_LIMIT);
    let choke_grinds = choked_grinds(recipe);
    let nearest = nearest_sample(recipe, target_seconds);
    let mut json = String::new();
    json.push('{');
    json.push_str("\"target_seconds\":");
    json.push_str(&fmt(target_seconds));
    json.push_str(",\"numeric_sample_count\":");
    json.push_str(&points.len().to_string());
    json.push_str(",\"model_sample_count\":");
    json.push_str(&model_points.len().to_string());

    if let Some((sample, _)) = nearest {
        json.push_str(",\"nearest\":{\"time\":");
        json_string_into(&mut json, &sample.time);
        json.push_str(",\"grind\":");
        json_string_into(&mut json, &sample.grind);
        json.push('}');
    } else {
        json.push_str(",\"nearest\":null");
    }

    if let Some((intercept, slope)) = theil_sen_model(&model_points) {
        let predicted_grind = (target_seconds - intercept) / slope;
        let curve_grind = exponential_predicted_grind(&model_points, target_seconds);
        let model_r2 = r_squared(&model_points, intercept, slope);
        let svg = render_graph_svg(
            recipe,
            &points,
            &model_points,
            &choke_grinds,
            target_seconds,
            predicted_grind,
            intercept,
            slope,
            model_r2,
        );
        json.push_str(",\"grind\":");
        json.push_str(&fmt(predicted_grind));
        json.push_str(",\"curve_grind\":");
        if let Some(curve_grind) = curve_grind {
            json.push_str(&fmt(curve_grind));
        } else {
            json.push_str("null");
        }
        json.push_str(",\"curve_model\":");
        if let Some((log_intercept, log_slope)) = exponential_time_model(&model_points) {
            json_string_into(
                &mut json,
                &format!(
                    "ln(time) = {} + {} * grind",
                    fmt(log_intercept),
                    fmt(log_slope)
                ),
            );
        } else {
            json.push_str("null");
        }
        json.push_str(",\"r_squared\":");
        json.push_str(&fmt(model_r2));
        json.push_str(",\"model\":\"time = ");
        json.push_str(&fmt(intercept));
        json.push_str(" + ");
        json.push_str(&fmt(slope));
        json.push_str(" * grind\"");
        json.push_str(",\"graph_svg\":");
        json_string_into(&mut json, &svg);
        json.push_str(",\"graph_error\":null");
        json.push_str(",\"note\":null");
    } else {
        json.push_str(",\"grind\":null,\"curve_grind\":null,\"curve_model\":null,\"r_squared\":null,\"model\":null,\"graph_svg\":null");
        json.push_str(",\"graph_error\":");
        if let Some(error) = theil_sen_fit_error(&model_points) {
            json_string_into(&mut json, &error);
        } else {
            json.push_str("null");
        }
        json.push_str(",\"note\":");
        json_string_into(&mut json, &prediction_note(&points));
    }

    json.push('}');
    json
}

fn prediction_note(points: &[(f64, f64)]) -> String {
    match points.len() {
        0 => "Log two shots with different grinds to unlock prediction.".to_string(),
        1 => "Log one more shot at a different grind to unlock prediction.".to_string(),
        _ => "Use different grind settings to unlock prediction.".to_string(),
    }
}

fn theil_sen_fit_error(points: &[(f64, f64)]) -> Option<String> {
    if points.len() < 2 {
        return None;
    }

    let varying_grind = points
        .windows(2)
        .any(|pair| pair[0].0.total_cmp(&pair[1].0) != std::cmp::Ordering::Equal);
    if !varying_grind {
        return Some(
            "Theil-Sen model could not be fitted because the local shots use the same grind setting."
                .to_string(),
        );
    }

    Some(
        "Theil-Sen model could not be fitted because the local shots do not show a usable grind/time relationship."
            .to_string(),
    )
}

fn nearest_sample(recipe: &Recipe, target_seconds: f64) -> Option<(&Sample, f64)> {
    recipe
        .samples
        .iter()
        .filter_map(|sample| {
            let shot_time = numeric_value(&sample.time);
            if is_number(&shot_time) && parse_number(&shot_time) > 0.0 {
                let seconds = parse_number(&shot_time);
                Some((sample, (seconds - target_seconds).abs()))
            } else {
                None
            }
        })
        .min_by(|(_, left), (_, right)| left.total_cmp(right))
}

fn parse_form_encoded(input: &str) -> Vec<(String, String)> {
    input
        .split('&')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let (key, value) = part.split_once('=').unwrap_or((part, ""));
            (url_decode(key), url_decode(value))
        })
        .collect()
}

fn form_value<'a>(params: &'a [(String, String)], key: &str) -> Option<&'a str> {
    params
        .iter()
        .find(|(candidate, _)| candidate == key)
        .map(|(_, value)| value.as_str())
}

fn url_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0usize;

    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                output.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                if let Ok(hex) = std::str::from_utf8(&bytes[index + 1..index + 3]) {
                    if let Ok(byte) = u8::from_str_radix(hex, 16) {
                        output.push(byte);
                        index += 3;
                        continue;
                    }
                }
                output.push(bytes[index]);
                index += 1;
            }
            byte => {
                output.push(byte);
                index += 1;
            }
        }
    }

    String::from_utf8_lossy(&output).into_owned()
}

fn json_error(message: &str) -> String {
    let mut json = String::from("{\"error\":");
    json_string_into(&mut json, message);
    json.push('}');
    json
}

fn json_string_into(output: &mut String, value: &str) {
    output.push('"');
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            character if character.is_control() => {
                output.push_str(&format!("\\u{:04x}", character as u32));
            }
            character => output.push(character),
        }
    }
    output.push('"');
}

fn web_app_html() -> &'static str {
    r###"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover">
  <title>Coffee Dial-In</title>
  <style>
    :root {
      color-scheme: dark;
      --bg: #0d0f12;
      --panel: #181b20;
      --panel-2: #20252c;
      --text: #f5f1e8;
      --muted: #aaa49a;
      --line: #343a42;
      --add: #d7b84f;
      --goal: #a6e22e;
      --goal-bg: rgba(166, 226, 46, 0.14);
      --goal-line: rgba(166, 226, 46, 0.46);
      --danger: #ff4d6d;
      --danger-text: #ffb3c1;
      --danger-bg: rgba(255, 77, 109, 0.14);
      --danger-line: rgba(255, 77, 109, 0.58);
      --shadow: 0 22px 60px rgba(0, 0, 0, 0.34);
    }

    * { box-sizing: border-box; }
    html { min-height: 100%; background: var(--bg); }
    body {
      min-height: 100%;
      margin: 0;
      font-family: Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
      color: var(--text);
      background:
        linear-gradient(180deg, rgba(255, 255, 255, 0.032), transparent 9rem),
        linear-gradient(115deg, rgba(215, 184, 79, 0.048), rgba(215, 184, 79, 0.016) 28rem, transparent 44rem),
        repeating-linear-gradient(90deg, rgba(255, 255, 255, 0.014) 0 1px, transparent 1px 80px),
        repeating-linear-gradient(0deg, rgba(255, 255, 255, 0.01) 0 1px, transparent 1px 80px),
        linear-gradient(180deg, #15171b 0%, #0f1114 54%, #0b0c0f 100%);
    }

    button, input, select {
      font: inherit;
      color: inherit;
    }

    .app {
      width: min(1180px, 100%);
      min-height: 100dvh;
      margin: 0 auto;
      padding: max(18px, env(safe-area-inset-top)) 16px max(28px, env(safe-area-inset-bottom));
    }

    .meta {
      color: var(--muted);
      font-size: 0.88rem;
    }

    .primary-layout {
      display: grid;
      grid-template-columns: minmax(280px, 0.68fr) minmax(0, 1.55fr);
      gap: 16px;
      align-items: stretch;
    }

    .panel {
      background: color-mix(in srgb, var(--panel) 94%, black);
      border: 1px solid var(--line);
      border-radius: 8px;
      box-shadow: var(--shadow);
    }

    .control-panel {
      position: sticky;
      top: 14px;
      display: grid;
      gap: 10px;
      align-content: start;
      padding: 0;
      overflow: visible;
      background: transparent;
      border: 0;
      box-shadow: none;
    }

    .control-section {
      display: grid;
      grid-template-rows: 48px auto;
      align-content: start;
      gap: 0;
      padding: 0;
      background: color-mix(in srgb, var(--panel) 94%, black);
      border: 1px solid var(--line);
      border-radius: 8px;
      box-shadow: var(--shadow);
      overflow: hidden;
    }

    .control-section + .control-section {
      background:
        linear-gradient(rgba(255, 255, 255, 0.018), rgba(255, 255, 255, 0.018)),
        color-mix(in srgb, var(--panel) 94%, black);
    }

    .section-title {
      display: flex;
      align-items: center;
      justify-content: space-between;
      gap: 12px;
      min-height: 48px;
      padding: 0 15px;
      border-bottom: 1px solid var(--line);
      background: rgba(255, 255, 255, 0.026);
    }

    .section-title h2 {
      margin: 0;
      font-size: 1.05rem;
      line-height: 1.15;
      letter-spacing: 0;
    }

    .section-title span {
      color: var(--muted);
      font-size: 0.82rem;
    }

    .section-body {
      display: grid;
      gap: 14px;
      padding: 15px;
    }

    .field {
      display: grid;
      gap: 7px;
    }

    .field.inline-setting {
      grid-template-columns: auto minmax(4.5rem, 6rem);
      justify-content: space-between;
      align-items: center;
      gap: 12px;
      min-height: 50px;
      border: 1px solid var(--line);
      border-radius: 8px;
      background:
        linear-gradient(90deg, rgba(166, 226, 46, 0.035), transparent 42%),
        var(--panel-2);
      padding: 0 12px 0 13px;
    }

    .field.inline-setting label {
      display: inline-flex;
      align-items: center;
      gap: 6px;
      white-space: nowrap;
      font-size: 0.98rem;
      font-weight: 900;
      color: #c9c5b9;
    }

    .field.inline-setting label svg {
      width: 20px;
      height: 20px;
      color: #d9d6cc;
      stroke: currentColor;
      flex: 0 0 auto;
    }

    .field.inline-setting input {
      width: 6rem;
      min-height: 38px;
      border-color: transparent;
      background: transparent;
      text-align: center;
      font-size: 2rem;
      font-weight: 500;
      color: #f8fafc;
      padding: 6px 4px;
    }

    .field.inline-setting input:focus {
      border-color: var(--goal);
      box-shadow: 0 0 0 3px var(--goal-bg);
    }

    label {
      color: var(--muted);
      font-size: 0.78rem;
      font-weight: 700;
      text-transform: uppercase;
      letter-spacing: 0.08em;
    }

    input, select {
      width: 100%;
      min-height: 50px;
      border: 1px solid var(--line);
      border-radius: 8px;
      background: var(--panel-2);
      padding: 12px 13px;
      outline: none;
    }

    select {
      padding-right: 42px;
      background-clip: padding-box;
    }

    input:focus, select:focus {
      border-color: var(--add);
      box-shadow: 0 0 0 3px rgba(215, 184, 79, 0.18);
    }

    .row {
      display: grid;
      grid-template-columns: 1fr 1fr;
      gap: 10px;
    }

    .button {
      display: inline-flex;
      align-items: center;
      justify-content: center;
      gap: 8px;
      min-height: 50px;
      border: 0;
      border-radius: 8px;
      background: var(--add);
      color: #102018;
      font-weight: 800;
      padding: 0 16px;
      cursor: pointer;
      touch-action: manipulation;
      position: relative;
      overflow: hidden;
    }

    .button svg {
      width: 18px;
      height: 18px;
      stroke: currentColor;
      flex: 0 0 auto;
    }

    .button.secondary {
      background: #2c333b;
      color: var(--text);
      border: 1px solid var(--line);
    }

    .button.compact {
      min-height: 38px;
      padding: 0 12px;
      font-size: 0.88rem;
    }

    .button.goal-action,
    .button.sample-action {
      background:
        linear-gradient(180deg, rgba(234, 255, 183, 0.09), rgba(166, 226, 46, 0.08) 42%, rgba(166, 226, 46, 0.16)),
        rgba(18, 28, 18, 0.92);
      color: #eaffb7;
      border: 1px solid rgba(166, 226, 46, 0.72);
      box-shadow:
        inset 0 1px 0 rgba(234, 255, 183, 0.16),
        inset 0 -1px 0 rgba(0, 0, 0, 0.22),
        0 9px 20px rgba(0, 0, 0, 0.26);
    }

    .button:not(:disabled)::after,
    .icon-button:not(:disabled)::after {
      content: "";
      position: absolute;
      inset: 0;
      transform: translateX(-135%);
      background: linear-gradient(
        110deg,
        transparent 0%,
        transparent 36%,
        rgba(217, 249, 157, 0.06) 45%,
        rgba(217, 249, 157, 0.18) 50%,
        rgba(217, 249, 157, 0.06) 55%,
        transparent 64%,
        transparent 100%
      );
      pointer-events: none;
      animation: button-shimmer 7.5s ease-in-out infinite;
    }

    .button > *,
    .icon-button > * {
      position: relative;
      z-index: 1;
    }

    .button.goal-action:hover,
    .button.goal-action:focus,
    .button.sample-action:hover,
    .button.sample-action:focus {
      background:
        linear-gradient(180deg, rgba(234, 255, 183, 0.14), rgba(166, 226, 46, 0.14) 42%, rgba(166, 226, 46, 0.22)),
        rgba(22, 35, 20, 0.96);
      border-color: rgba(166, 226, 46, 0.86);
      box-shadow:
        inset 0 1px 0 rgba(234, 255, 183, 0.22),
        inset 0 -1px 0 rgba(0, 0, 0, 0.22),
        0 0 0 3px var(--goal-bg),
        0 10px 22px rgba(0, 0, 0, 0.24);
    }

    .icon-button {
      display: inline-grid;
      place-items: center;
      width: 38px;
      height: 38px;
      border: 1px solid var(--line);
      border-radius: 8px;
      background: #2c333b;
      color: var(--muted);
      cursor: pointer;
      touch-action: manipulation;
      position: relative;
      overflow: hidden;
    }

    .icon-button.recipe-delete {
      width: 100%;
      height: auto;
      min-height: 50px;
    }

    .icon-button svg {
      width: 18px;
      height: 18px;
      stroke: currentColor;
    }

    .icon-button.danger:hover,
    .icon-button.danger:focus {
      color: #ffd6df;
      border-color: rgba(255, 77, 109, 0.78);
      background: rgba(255, 77, 109, 0.18);
    }

    .icon-button.danger {
      color: var(--danger-text);
      border-color: var(--danger-line);
      background: var(--danger-bg);
    }

    .icon-button.refresh {
      color: #d9f99d;
      border-color: rgba(166, 226, 46, 0.45);
      background: rgba(72, 104, 31, 0.45);
    }

    .icon-button.refresh:hover,
    .icon-button.refresh:focus {
      color: #f0ffd0;
      border-color: rgba(166, 226, 46, 0.72);
      background: rgba(91, 133, 37, 0.58);
      box-shadow: 0 0 0 3px var(--goal-bg);
    }

    .icon-button:disabled {
      opacity: 0.45;
      cursor: not-allowed;
    }

    .button:active { transform: translateY(1px); }

    @keyframes button-shimmer {
      0%, 68% {
        transform: translateX(-135%);
      }
      82%, 100% {
        transform: translateX(135%);
      }
    }

    @media (prefers-reduced-motion: reduce) {
      .button::after,
      .icon-button::after {
        animation: none;
        display: none;
      }
    }

    .row > .button {
      align-self: end;
    }

    .modal-form > .button,
    #sampleForm > .button {
      grid-column: 1 / -1;
    }

    .recipe-actions {
      display: grid;
      grid-template-columns: 3fr 1fr;
      gap: 10px;
      align-items: stretch;
    }

    .recipe-actions > .button,
    .recipe-actions > .icon-button {
      height: 100%;
    }

    .shot-actions {
      display: grid;
      margin-top: 4px;
    }

    .add-sample-button {
      width: 38px;
      height: 38px;
      color: #eaffb7;
      border-color: rgba(166, 226, 46, 0.72);
      background:
        linear-gradient(180deg, rgba(234, 255, 183, 0.09), rgba(166, 226, 46, 0.08) 42%, rgba(166, 226, 46, 0.16)),
        rgba(18, 28, 18, 0.92);
    }

    .add-sample-button:hover,
    .add-sample-button:focus {
      color: #f0ffd0;
      border-color: rgba(166, 226, 46, 0.86);
      background:
        linear-gradient(180deg, rgba(234, 255, 183, 0.14), rgba(166, 226, 46, 0.14) 42%, rgba(166, 226, 46, 0.22)),
        rgba(22, 35, 20, 0.96);
      box-shadow: 0 0 0 3px var(--goal-bg);
    }

    .prediction {
      display: grid;
      grid-template-columns: 1fr auto;
      gap: 12px;
      align-items: end;
      padding: 16px;
      background: linear-gradient(135deg, rgba(166, 226, 46, 0.14), #191f20);
      border: 1px solid var(--goal-line);
      border-radius: 8px;
      cursor: default;
    }

    .prediction.is-actionable {
      cursor: pointer;
    }

    .prediction.is-actionable:hover,
    .prediction.is-actionable:focus-within {
      border-color: rgba(166, 226, 46, 0.72);
      box-shadow: 0 0 0 3px var(--goal-bg);
    }

    .prediction .label {
      color: var(--muted);
      font-size: 0.78rem;
      font-weight: 800;
      text-transform: uppercase;
      letter-spacing: 0.08em;
    }

    .prediction .grind {
      font-size: 3.2rem;
      line-height: 0.9;
      font-weight: 900;
      letter-spacing: 0;
    }

    .prediction .curve-grind {
      margin-top: 8px;
      color: #fbbf24;
      font-size: 1.02rem;
      font-weight: 900;
      letter-spacing: 0;
    }

    .prediction .target {
      color: var(--goal);
      font-weight: 800;
      text-align: right;
      white-space: nowrap;
    }

    .graph-panel {
      display: grid;
      grid-template-rows: auto minmax(0, 1fr);
      overflow: hidden;
      min-height: 100%;
    }

    .graph-head {
      display: flex;
      justify-content: space-between;
      gap: 18px;
      align-items: center;
      min-height: 48px;
      padding: 0 16px 0 18px;
      border-bottom: 1px solid var(--line);
      background: rgba(255, 255, 255, 0.026);
    }

    .graph-head h2 {
      margin: 0;
      font-size: 1.05rem;
      font-weight: 800;
      letter-spacing: 0;
    }

    .graph-meta {
      display: flex;
      align-items: center;
      justify-content: flex-end;
      gap: 14px;
      text-align: right;
    }

    .shot-meter {
      display: inline-flex;
      align-items: center;
      gap: 8px;
      min-height: 38px;
      padding: 0 10px;
      border: 1px solid var(--goal-line);
      border-radius: 8px;
      background: rgba(166, 226, 46, 0.08);
      color: #d9f99d;
      font-size: 0.92rem;
      font-weight: 800;
      text-transform: uppercase;
      white-space: nowrap;
    }

    .shot-meter.is-empty {
      border-color: rgba(166, 226, 46, 0.28);
      background: rgba(166, 226, 46, 0.045);
      color: rgba(217, 249, 157, 0.78);
    }

    .shot-meter.is-hidden {
      display: none;
    }

    .shot-notches {
      display: grid;
      grid-template-columns: repeat(6, 12px);
      gap: 4px;
      align-items: end;
      height: 18px;
    }

    .shot-notch {
      width: 12px;
      height: 8px;
      border-radius: 2px;
      background: rgba(166, 226, 46, 0.08);
      border: 1px solid rgba(166, 226, 46, 0.14);
    }

    .shot-notch:nth-child(2) { height: 10px; }
    .shot-notch:nth-child(3) { height: 12px; }
    .shot-notch:nth-child(4) { height: 14px; }
    .shot-notch:nth-child(5) { height: 16px; }
    .shot-notch:nth-child(6) { height: 18px; }

    .shot-notch.is-filled {
      background: var(--goal);
      border-color: rgba(166, 226, 46, 0.78);
      box-shadow: 0 0 10px rgba(166, 226, 46, 0.22);
    }

    .graph-body {
      display: grid;
      grid-template-columns: minmax(0, 1fr) minmax(220px, 260px);
      height: 560px;
      min-height: 0;
    }

    .graph-main {
      display: grid;
      grid-template-rows: auto minmax(0, 1fr);
      min-height: 0;
      background: #15181d;
    }

    .graph-legend {
      display: flex;
      flex-wrap: wrap;
      align-items: center;
      gap: 8px 14px;
      min-height: 38px;
      padding: 8px 12px 4px;
      color: var(--muted);
      font-size: 0.72rem;
      font-weight: 800;
      text-transform: uppercase;
      letter-spacing: 0.06em;
    }

    .legend-item {
      display: inline-flex;
      align-items: center;
      gap: 6px;
      white-space: nowrap;
    }

    .legend-line {
      width: 24px;
      height: 0;
      border-top: 3px solid currentColor;
      border-radius: 999px;
    }

    .legend-line.is-solid { color: #60a5fa; }
    .legend-line.is-dotted {
      width: 28px;
      border-top-style: dotted;
      color: #fbbf24;
    }
    .legend-line.is-target {
      width: 28px;
      border-top-style: dashed;
      color: #a6e22e;
    }

    .legend-dot {
      width: 10px;
      height: 10px;
      border-radius: 50%;
      background: currentColor;
      box-shadow: 0 0 0 2px #0f172a;
    }

    .legend-dot.is-sample { color: #64748b; }
    .legend-dot.is-model { color: #2dd4bf; }

    .legend-zone {
      width: 20px;
      height: 10px;
      border-left: 3px dotted var(--danger);
      background: rgba(255, 77, 109, 0.16);
    }

    .graph-wrap {
      min-height: 280px;
      padding: 8px 10px 10px;
      overflow: hidden;
      background:
        linear-gradient(180deg, rgba(255, 255, 255, 0.014), transparent 7rem),
        #15181d;
      display: grid;
      align-items: stretch;
    }

    .graph-wrap svg {
      display: block;
      width: 100%;
      height: 100%;
      min-height: 0;
      border-radius: 6px;
    }

    .samples {
      display: grid;
      gap: 8px;
      grid-template-rows: auto auto minmax(0, 1fr);
      min-height: 0;
      overflow: hidden;
      padding: 12px 16px 16px;
      border-left: 1px solid var(--line);
      background: rgba(13, 15, 18, 0.36);
    }

    .samples-head {
      display: flex;
      align-items: center;
      justify-content: space-between;
      gap: 10px;
      min-width: 0;
    }

    .sample-list-head,
    .sample-list {
      min-width: 0;
    }

    .sample-list-head {
      display: grid;
      grid-template-columns: minmax(0, 1fr) minmax(0, 1fr) 34px;
      gap: 10px;
      padding: 0 10px;
      color: var(--muted);
      font-size: 0.72rem;
      font-weight: 800;
      text-transform: uppercase;
      letter-spacing: 0.08em;
    }

    .sample-list {
      display: grid;
      align-content: start;
      gap: 6px;
      min-height: 0;
      overflow-y: auto;
      overflow-x: hidden;
      padding-right: 2px;
    }

    .sample {
      display: grid;
      grid-template-columns: minmax(0, 1fr) minmax(0, 1fr) 34px;
      align-items: center;
      gap: 10px;
      min-height: 38px;
      padding: 5px 6px 5px 10px;
      background: var(--panel-2);
      border: 1px solid var(--line);
      border-radius: 8px;
    }

    .sample-cell {
      display: inline-flex;
      align-items: center;
      gap: 7px;
      min-width: 0;
      color: var(--text);
      font-weight: 800;
      line-height: 1;
    }

    .sample-cell svg {
      width: 16px;
      height: 16px;
      flex: 0 0 auto;
      color: var(--muted);
      stroke: currentColor;
    }

    .sample-cell span {
      overflow: hidden;
      text-overflow: ellipsis;
      white-space: nowrap;
      min-width: 0;
    }

    .sample .icon-button {
      width: 34px;
      height: 34px;
    }

    .sample.is-choked {
      border-color: var(--danger-line);
      background:
        linear-gradient(rgba(255, 77, 109, 0.08), rgba(255, 77, 109, 0.08)),
        var(--panel-2);
    }

    .sample.is-choked .sample-cell:first-child {
      color: var(--danger-text);
    }

    dialog.modal {
      width: min(440px, calc(100vw - 24px));
      max-height: calc(100dvh - max(44px, env(safe-area-inset-top)) - 16px);
      border: 1px solid var(--line);
      border-radius: 8px;
      background: var(--panel);
      color: var(--text);
      padding: 0;
      box-shadow: var(--shadow);
      overflow: auto;
      position: fixed;
      top: max(44px, env(safe-area-inset-top));
      bottom: auto;
      left: 50%;
      right: auto;
      transform: translateX(-50%);
      margin: 0;
    }

    dialog.modal::backdrop {
      background: rgba(0, 0, 0, 0.58);
      backdrop-filter: blur(2px);
    }

    .modal-head {
      display: flex;
      align-items: center;
      justify-content: space-between;
      gap: 12px;
      padding: 14px 16px;
      border-bottom: 1px solid var(--line);
    }

    .modal-head h2 {
      margin: 0;
      font-size: 1rem;
      letter-spacing: 0;
    }

    .modal-form {
      display: grid;
      gap: 14px;
      padding: 16px;
    }

    .model-choice-field {
      display: none;
      gap: 10px;
    }

    .modal-form.is-model-choice .manual-grind-field {
      display: none;
    }

    .modal-form.is-model-choice .model-choice-field {
      display: grid;
    }

    .model-choice-buttons {
      display: grid;
      grid-template-columns: 1fr 1fr;
      gap: 10px;
    }

    .model-choice-button {
      display: grid;
      gap: 4px;
      justify-items: start;
      width: 100%;
      height: auto;
      min-height: 64px;
      padding: 10px 12px;
      color: #eaffb7;
      text-align: left;
      border-color: rgba(166, 226, 46, 0.48);
      background:
        linear-gradient(180deg, rgba(234, 255, 183, 0.06), rgba(166, 226, 46, 0.06)),
        rgba(18, 28, 18, 0.72);
    }

    .model-choice-button .model-label {
      color: var(--muted);
      font-size: 0.72rem;
      font-weight: 900;
      text-transform: uppercase;
      letter-spacing: 0.08em;
    }

    .model-choice-button .model-value {
      font-size: 1.35rem;
      font-weight: 900;
      line-height: 1;
      letter-spacing: 0;
    }

    .model-choice-button.is-selected {
      border-color: rgba(166, 226, 46, 0.88);
      box-shadow: 0 0 0 3px var(--goal-bg);
      background:
        linear-gradient(180deg, rgba(234, 255, 183, 0.14), rgba(166, 226, 46, 0.16)),
        rgba(22, 35, 20, 0.96);
    }

    .model-choice-button.curve-model {
      color: #fbbf24;
      border-color: rgba(251, 191, 36, 0.48);
      background:
        linear-gradient(180deg, rgba(251, 191, 36, 0.08), rgba(251, 191, 36, 0.04)),
        rgba(32, 25, 12, 0.8);
    }

    .model-choice-button.curve-model.is-selected {
      border-color: rgba(251, 191, 36, 0.88);
      box-shadow: 0 0 0 3px rgba(251, 191, 36, 0.16);
    }

    .toggle-field {
      display: flex;
      align-items: center;
      justify-content: space-between;
      gap: 12px;
      min-height: 48px;
      padding: 0 13px;
      border: 1px solid var(--danger-line);
      border-radius: 8px;
      background: var(--danger-bg);
    }

    .toggle-field label {
      color: var(--danger-text);
    }

    .toggle-field input {
      width: 22px;
      min-height: 22px;
      accent-color: var(--danger);
    }

    .empty, .status {
      color: var(--muted);
      padding: 22px;
      text-align: center;
    }

    .graph-wrap .empty {
      min-height: 380px;
      display: grid;
      place-items: center;
      padding: 0 18px;
      color: rgba(170, 164, 154, 0.34);
      font-size: 3rem;
      font-weight: 800;
      letter-spacing: 0.08em;
      text-shadow: 0 0 18px rgba(170, 164, 154, 0.035);
    }

    .graph-wrap .graph-error {
      min-height: 380px;
      display: grid;
      align-content: center;
      justify-items: center;
      gap: 10px;
      padding: 24px;
      color: #ffd6df;
      text-align: center;
      border: 1px solid var(--danger-line);
      border-radius: 8px;
      background: var(--danger-bg);
    }

    .graph-error strong {
      font-size: 1rem;
      letter-spacing: 0;
    }

    .graph-error span {
      max-width: 34rem;
      color: var(--danger-text);
      line-height: 1.35;
    }

    .toast {
      position: fixed;
      left: 16px;
      right: 16px;
      bottom: max(16px, env(safe-area-inset-bottom));
      z-index: 5;
      display: none;
      max-width: 720px;
      margin: 0 auto;
      padding: 12px 14px;
      border-radius: 8px;
      background: #262b32;
      border: 1px solid var(--line);
      box-shadow: var(--shadow);
      color: var(--text);
    }

    .toast.is-visible { display: block; }
    .toast.is-error { border-color: rgba(255, 77, 109, 0.68); color: #ffd6df; }

    @media (max-width: 820px) {
      .app { padding-inline: 12px; }
      .primary-layout {
        grid-template-columns: 1fr;
        align-items: start;
      }
      .control-panel {
        position: static;
        box-shadow: none;
      }
      .section-title { padding: 0 12px; }
      .section-body,
      .control-section:first-child .section-body,
      .control-section + .control-section .section-body {
        padding: 14px 12px;
      }
      .prediction .grind { font-size: 2.8rem; }
      .graph-panel { min-height: 0; }
      .graph-panel { grid-template-rows: auto auto; }
      .graph-body {
        grid-template-columns: 1fr;
        height: auto;
      }
      .graph-legend {
        gap: 7px 10px;
        min-height: 0;
        padding: 8px 10px 2px;
      }
      .graph-wrap { padding: 6px; }
      .graph-wrap svg {
        height: auto;
        min-height: 0;
      }
      .samples {
        max-height: 180px;
        min-height: 0;
        padding-inline: 12px;
        border-left: 0;
        border-top: 1px solid var(--line);
      }
    }

    @media (max-width: 520px) {
      .row {
        grid-template-columns: 1fr;
      }
      .field.inline-setting {
        grid-template-columns: auto minmax(4.5rem, 6rem);
      }
      .prediction {
        grid-template-columns: 1fr;
      }
      .prediction .target {
        text-align: left;
      }
      .graph-head {
        display: grid;
        min-height: 0;
        padding: 12px 16px;
      }
      .graph-meta {
        justify-content: space-between;
        text-align: left;
      }
      .button, input, select {
        min-height: 54px;
      }
      dialog.modal {
        width: min(440px, calc(100vw - 20px));
        top: max(36px, env(safe-area-inset-top));
        max-height: calc(100dvh - max(36px, env(safe-area-inset-top)) - 12px);
      }
    }
  </style>
</head>
<body>
  <main class="app">
    <section class="primary-layout">
      <div class="panel control-panel">
        <section class="control-section">
          <div class="section-title">
            <h2>Recipe</h2>
          </div>
          <div class="section-body">
            <div class="field">
              <select id="recipeSelect" aria-label="Recipe"></select>
            </div>

            <div class="recipe-actions">
              <button class="button goal-action" id="toggleRecipeButton" type="button">
                <svg viewBox="0 0 24 24" fill="none" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                  <rect width="8" height="4" x="8" y="2" rx="1" ry="1"/>
                  <path d="M16 4h2a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2h2"/>
                  <path d="M9 14h6"/>
                  <path d="M12 11v6"/>
                </svg>
                <span>New Recipe</span>
              </button>
              <button class="icon-button danger recipe-delete" id="deleteRecipeButton" type="button" aria-label="Delete recipe">
                <svg viewBox="0 0 24 24" fill="none" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                  <path d="M3 6h18"/>
                  <path d="M8 6V4h8v2"/>
                  <path d="M19 6l-1 14H6L5 6"/>
                  <path d="M10 11v5"/>
                  <path d="M14 11v5"/>
                </svg>
              </button>
            </div>
          </div>
        </section>

        <section class="control-section">
          <div class="section-title">
            <h2>Shot</h2>
          </div>

          <div class="section-body">
            <div class="field inline-setting">
              <label for="targetTime">
                <svg viewBox="0 0 24 24" fill="none" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                  <line x1="10" x2="14" y1="2" y2="2"/>
                  <line x1="12" x2="15" y1="14" y2="11"/>
                  <circle cx="12" cy="14" r="8"/>
                </svg>
                <span>Target</span>
              </label>
              <input id="targetTime" inputmode="decimal" value="30" autocomplete="off">
            </div>

            <div class="prediction" id="predictionBox">
              <div>
                <div class="label">Linear grind</div>
                <div class="grind" id="predictedGrind">--</div>
                <div class="curve-grind" id="curveGrind">Curve --</div>
              </div>
              <div class="target" id="targetMeta">30s</div>
            </div>
          </div>
        </section>
      </div>

      <div class="panel graph-panel">
        <div class="graph-head">
          <h2 id="graphTitle">Shot graph</h2>
          <div class="graph-meta">
            <div class="shot-meter" id="shotMeter" aria-label="No shots logged">
              <span class="shot-notches" aria-hidden="true">
                <span class="shot-notch"></span>
                <span class="shot-notch"></span>
                <span class="shot-notch"></span>
                <span class="shot-notch"></span>
                <span class="shot-notch"></span>
                <span class="shot-notch"></span>
              </span>
              <span id="shotMeterText">0 shots</span>
            </div>
            <button class="icon-button refresh" id="refreshButton" type="button" aria-label="Refresh graph">
              <svg viewBox="0 0 24 24" fill="none" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                <path d="M3 12a9 9 0 0 1 15.74-5.74L21 8"/>
                <path d="M21 3v5h-5"/>
                <path d="M21 12a9 9 0 0 1-15.74 5.74L3 16"/>
                <path d="M3 21v-5h5"/>
              </svg>
            </button>
          </div>
        </div>
        <div class="graph-body">
          <div class="graph-main">
            <div class="graph-legend" aria-label="Graph legend">
              <span class="legend-item"><span class="legend-line is-solid"></span> Linear</span>
              <span class="legend-item"><span class="legend-line is-dotted"></span> Curve</span>
              <span class="legend-item"><span class="legend-line is-target"></span> Target</span>
              <span class="legend-item"><span class="legend-dot is-model"></span> Model shots</span>
              <span class="legend-item"><span class="legend-dot is-sample"></span> Other shots</span>
              <span class="legend-item"><span class="legend-zone"></span> Choke</span>
            </div>
            <div class="graph-wrap" id="graphWrap">
              <div class="empty">--</div>
            </div>
          </div>
          <div class="samples">
            <div class="samples-head">
              <div class="meta" id="sampleMeta"></div>
              <button class="icon-button add-sample-button" id="openSampleButton" type="button" aria-label="Add shot">
                <svg viewBox="0 0 24 24" fill="none" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                  <path d="M12 5v14"/>
                  <path d="M5 12h14"/>
                </svg>
              </button>
            </div>
            <div class="sample-list-head" aria-hidden="true">
              <span>Time</span>
              <span>Grind</span>
              <span></span>
            </div>
            <div class="sample-list" id="sampleList"></div>
          </div>
        </div>
      </div>
    </section>
  </main>
  <dialog class="modal" id="recipeDialog">
    <div class="modal-head">
      <h2>New Recipe</h2>
      <button class="icon-button" type="button" data-close-dialog="recipeDialog" aria-label="Close recipe dialog">
        <svg viewBox="0 0 24 24" fill="none" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
          <path d="M18 6L6 18"/>
          <path d="M6 6l12 12"/>
        </svg>
      </button>
    </div>
    <form id="recipeForm" class="modal-form" method="dialog">
      <div class="field">
        <label for="recipeName">Recipe name</label>
        <input id="recipeName" name="recipe" autocomplete="off" required>
      </div>
      <div class="row">
        <div class="field">
          <label for="recipeDose">Dose grams</label>
          <input id="recipeDose" name="dose" inputmode="decimal" autocomplete="off" required>
        </div>
        <div class="field">
          <label for="recipeShotWeight">Shot grams</label>
          <input id="recipeShotWeight" name="shot_weight" inputmode="decimal" autocomplete="off" required>
        </div>
      </div>
      <button class="button goal-action" type="submit">
        <svg viewBox="0 0 24 24" fill="none" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
          <rect width="8" height="4" x="8" y="2" rx="1" ry="1"/>
          <path d="M16 4h2a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2h2"/>
          <path d="M9 14h6"/>
          <path d="M12 11v6"/>
        </svg>
        <span>Create Recipe</span>
      </button>
    </form>
  </dialog>
  <dialog class="modal" id="sampleDialog">
    <div class="modal-head">
      <h2>Add Shot</h2>
      <button class="icon-button" type="button" data-close-dialog="sampleDialog" aria-label="Close shot dialog">
        <svg viewBox="0 0 24 24" fill="none" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
          <path d="M18 6L6 18"/>
          <path d="M6 6l12 12"/>
        </svg>
      </button>
    </div>
    <form id="sampleForm" class="modal-form" method="dialog">
      <div class="field">
        <label for="shotTime">Shot time</label>
        <input id="shotTime" name="time" inputmode="decimal" autocomplete="off" required>
      </div>
      <div class="toggle-field">
        <label for="chokedShot">Choked</label>
        <input id="chokedShot" name="choked" type="checkbox" value="1">
      </div>
      <div class="field manual-grind-field" id="manualGrindField">
        <label for="grind">Grind</label>
        <input id="grind" name="grind" inputmode="decimal" autocomplete="off" required>
      </div>
      <div class="field model-choice-field" id="modelChoiceField">
        <label>Grind model</label>
        <div class="model-choice-buttons">
          <button class="icon-button model-choice-button linear-model" id="linearModelButton" type="button" data-model="linear">
            <span class="model-label">Linear</span>
            <span class="model-value" id="linearModelValue">--</span>
          </button>
          <button class="icon-button model-choice-button curve-model" id="curveModelButton" type="button" data-model="curve">
            <span class="model-label">Curve</span>
            <span class="model-value" id="curveModelValue">--</span>
          </button>
        </div>
      </div>
      <button class="button sample-action" type="submit">
        <svg viewBox="0 0 24 24" fill="none" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
          <path d="M6 2v2"/>
          <path d="M10 2v2"/>
          <path d="M14 2v2"/>
          <path d="M16 8a1 1 0 0 1 1 1v8a4 4 0 0 1-4 4H7a4 4 0 0 1-4-4V9a1 1 0 0 1 1-1h14a4 4 0 1 1 0 8h-1"/>
        </svg>
        <span>Add Shot</span>
      </button>
    </form>
  </dialog>
  <div class="toast" id="toast"></div>

  <script>
    const state = { selectedRecipe: "", targetTime: "30", recipes: [] };

    const els = {
      recipeSelect: document.querySelector("#recipeSelect"),
      targetTime: document.querySelector("#targetTime"),
      predictedGrind: document.querySelector("#predictedGrind"),
      curveGrind: document.querySelector("#curveGrind"),
      predictionBox: document.querySelector("#predictionBox"),
      targetMeta: document.querySelector("#targetMeta"),
      shotTime: document.querySelector("#shotTime"),
      chokedShot: document.querySelector("#chokedShot"),
      grind: document.querySelector("#grind"),
      graphWrap: document.querySelector("#graphWrap"),
      graphTitle: document.querySelector("#graphTitle"),
      sampleMeta: document.querySelector("#sampleMeta"),
      sampleList: document.querySelector("#sampleList"),
      sampleForm: document.querySelector("#sampleForm"),
      recipeForm: document.querySelector("#recipeForm"),
      sampleDialog: document.querySelector("#sampleDialog"),
      recipeDialog: document.querySelector("#recipeDialog"),
      linearModelButton: document.querySelector("#linearModelButton"),
      curveModelButton: document.querySelector("#curveModelButton"),
      linearModelValue: document.querySelector("#linearModelValue"),
      curveModelValue: document.querySelector("#curveModelValue"),
      openSampleButton: document.querySelector("#openSampleButton"),
      shotMeter: document.querySelector("#shotMeter"),
      shotMeterText: document.querySelector("#shotMeterText"),
      deleteRecipeButton: document.querySelector("#deleteRecipeButton"),
      toast: document.querySelector("#toast")
    };

    function params(values) {
      return new URLSearchParams(values);
    }

    async function api(path, options = {}) {
      const response = await fetch(path, options);
      const data = await response.json();
      if (!response.ok || data.error) {
        throw new Error(data.error || "Request failed");
      }
      return data;
    }

    async function loadState() {
      const query = params({ recipe: state.selectedRecipe, time: els.targetTime.value || state.targetTime });
      const data = await api(`/api/state?${query}`);
      render(data);
    }

    function render(data) {
      state.recipes = data.recipes;
      state.selectedRecipe = data.selected_recipe || "";
      state.targetTime = data.target_time;

      els.recipeSelect.innerHTML = "";
      for (const recipe of data.recipes) {
        const option = document.createElement("option");
        option.value = recipe.name;
        option.textContent = `${recipe.name}, Dose: ${grams(recipe.dose_weight_g)}, Out: ${grams(recipe.shot_weight_g)}`;
        option.selected = recipe.name === state.selectedRecipe;
        els.recipeSelect.append(option);
      }

      const recipe = data.recipes.find(item => item.name === state.selectedRecipe);
      els.targetTime.value = Math.round(Number(data.target_time));
      els.graphTitle.textContent = recipe ? recipe.name : "Shot graph";
      renderShotMeter(recipe);
      els.deleteRecipeButton.disabled = !recipe;
      els.openSampleButton.disabled = !recipe;

      const prediction = data.prediction;
      if (prediction && prediction.grind !== null) {
        els.predictedGrind.textContent = Number(prediction.grind).toFixed(2);
        els.curveGrind.textContent = prediction.curve_grind !== null
          ? `Curve ${Number(prediction.curve_grind).toFixed(2)}`
          : "Curve --";
        els.predictionBox.dataset.predictedGrind = Number(prediction.grind).toFixed(2);
        els.predictionBox.dataset.curveGrind = prediction.curve_grind !== null
          ? Number(prediction.curve_grind).toFixed(2)
          : "";
        els.predictionBox.classList.add("is-actionable");
        els.targetMeta.textContent = `${Math.round(Number(prediction.target_seconds))}s target`;
        els.graphWrap.innerHTML = prediction.graph_svg;
      } else {
        els.predictedGrind.textContent = "--";
        els.curveGrind.textContent = "Curve --";
        els.predictionBox.dataset.predictedGrind = "";
        els.predictionBox.dataset.curveGrind = "";
        els.predictionBox.classList.remove("is-actionable");
        els.targetMeta.textContent = `${Math.round(Number(data.target_time))}s target`;
        if (prediction?.graph_error) {
          els.graphWrap.innerHTML = `<div class="graph-error"><strong>Graph unavailable</strong><span>${escapeHtml(prediction.graph_error)}</span></div>`;
        } else {
          els.graphWrap.innerHTML = `<div class="empty">--</div>`;
        }
      }

      renderSamples(recipe);
    }

    function renderShotMeter(recipe) {
      const count = recipe ? Number(recipe.sample_count) : 0;
      const filled = Math.min(count, 6);
      const label = count === 1 ? "1 shot" : `${count} shots`;
      els.shotMeterText.textContent = label;
      els.shotMeter.setAttribute("aria-label", `${label} logged`);
      els.shotMeter.classList.toggle("is-hidden", !recipe);
      els.shotMeter.classList.toggle("is-empty", count === 0);
      els.shotMeter.querySelectorAll(".shot-notch").forEach((notch, index) => {
        notch.classList.toggle("is-filled", index < filled);
      });
    }

    function renderSamples(recipe) {
      els.sampleList.innerHTML = "";
      if (!recipe) {
        els.sampleMeta.textContent = "";
        return;
      }
      const sampleCount = Number(recipe.sample_count);
      els.sampleMeta.textContent = sampleCount === 1 ? "1 logged shot" : `${sampleCount} logged shots`;
      if (sampleCount === 0) {
        els.sampleList.innerHTML = `<div class="empty">No shots yet</div>`;
        return;
      }
      for (const sample of [...recipe.samples].reverse()) {
        const row = document.createElement("div");
        const timeLabel = sample.choked ? "Choked" : sample.time;
        row.className = sample.choked ? "sample is-choked" : "sample";
        row.innerHTML = `<span class="sample-cell"><svg viewBox="0 0 24 24" fill="none" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><line x1="10" x2="14" y1="2" y2="2"/><line x1="12" x2="15" y1="14" y2="11"/><circle cx="12" cy="14" r="8"/></svg><span>${escapeHtml(timeLabel)}</span></span><span class="sample-cell"><svg viewBox="0 0 24 24" fill="none" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M3.34 19a10 10 0 1 1 17.32 0"/><path d="m12 14 4-4"/></svg><span>${escapeHtml(sample.grind)}</span></span><button class="icon-button danger delete-sample" type="button" data-sample-index="${sample.index}" data-sample-label="${escapeHtml(`${timeLabel} at grind ${sample.grind}`)}" aria-label="Delete shot"><svg viewBox="0 0 24 24" fill="none" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M3 6h18"/><path d="M8 6V4h8v2"/><path d="M19 6l-1 14H6L5 6"/><path d="M10 11v5"/><path d="M14 11v5"/></svg></button>`;
        els.sampleList.append(row);
      }
    }

    function escapeHtml(value) {
      return String(value).replace(/[&<>"']/g, character => ({
        "&": "&amp;",
        "<": "&lt;",
        ">": "&gt;",
        '"': "&quot;",
        "'": "&#039;"
      }[character]));
    }

    function grams(value) {
      const text = String(value || "").trim();
      return text.endsWith("g") ? text : `${text}g`;
    }

    function showToast(message, isError = false) {
      els.toast.textContent = message;
      els.toast.className = `toast is-visible${isError ? " is-error" : ""}`;
      clearTimeout(showToast.timeout);
      showToast.timeout = setTimeout(() => {
        els.toast.className = "toast";
      }, 2600);
    }

    function openDialog(dialog, focusTarget) {
      if (!dialog.open) {
        dialog.showModal();
      }
      requestAnimationFrame(() => focusTarget?.focus());
    }

    function fillPredictedGrind() {
      const predictedGrind = els.predictionBox.dataset.predictedGrind;
      if (!predictedGrind) {
        return false;
      }
      els.grind.value = predictedGrind;
      return true;
    }

    function resetModelChoice() {
      els.linearModelButton.classList.remove("is-selected");
      els.curveModelButton.classList.remove("is-selected");
      els.linearModelButton.setAttribute("aria-pressed", "false");
      els.curveModelButton.setAttribute("aria-pressed", "false");
    }

    function syncChokedShot() {
      const choked = els.chokedShot.checked;
      els.shotTime.required = !choked;
      els.shotTime.disabled = choked;
      if (choked) {
        els.shotTime.value = "";
      }
    }

    function openManualSampleDialog() {
      els.sampleForm.classList.remove("is-model-choice");
      els.chokedShot.checked = false;
      syncChokedShot();
      els.grind.required = true;
      resetModelChoice();
      fillPredictedGrind();
      openDialog(els.sampleDialog, els.shotTime);
    }

    function openModelChoiceSampleDialog() {
      const linearGrind = els.predictionBox.dataset.predictedGrind;
      const curveGrind = els.predictionBox.dataset.curveGrind;
      if (!linearGrind) {
        return false;
      }

      els.sampleForm.reset();
      syncChokedShot();
      els.grind.required = false;
      els.grind.value = "";
      resetModelChoice();
      els.sampleForm.classList.add("is-model-choice");
      els.linearModelButton.dataset.grind = linearGrind;
      els.curveModelButton.dataset.grind = curveGrind || "";
      els.linearModelValue.textContent = linearGrind;
      els.curveModelValue.textContent = curveGrind || "--";
      els.curveModelButton.disabled = !curveGrind;
      openDialog(els.sampleDialog, els.shotTime);
      return true;
    }

    function chooseModelGrind(button) {
      const grind = button.dataset.grind;
      if (!grind) {
        return;
      }
      els.grind.value = grind;
      resetModelChoice();
      button.classList.add("is-selected");
      button.setAttribute("aria-pressed", "true");
    }

    els.recipeSelect.addEventListener("change", () => {
      state.selectedRecipe = els.recipeSelect.value;
      loadState().catch(error => showToast(error.message, true));
    });

    els.targetTime.addEventListener("change", () => {
      loadState().catch(error => showToast(error.message, true));
    });

    document.querySelector("#refreshButton").addEventListener("click", () => {
      loadState().catch(error => showToast(error.message, true));
    });

    els.predictionBox.addEventListener("click", () => {
      if (!openModelChoiceSampleDialog()) {
        return;
      }
    });

    document.querySelector("#toggleRecipeButton").addEventListener("click", () => {
      openDialog(els.recipeDialog, document.querySelector("#recipeName"));
    });

    els.openSampleButton.addEventListener("click", () => {
      openManualSampleDialog();
    });

    els.linearModelButton.addEventListener("click", () => chooseModelGrind(els.linearModelButton));
    els.curveModelButton.addEventListener("click", () => chooseModelGrind(els.curveModelButton));
    els.chokedShot.addEventListener("change", syncChokedShot);

    document.querySelectorAll("[data-close-dialog]").forEach(button => {
      button.addEventListener("click", () => {
        document.querySelector(`#${button.dataset.closeDialog}`)?.close();
      });
    });

    els.deleteRecipeButton.addEventListener("click", async () => {
      if (!state.selectedRecipe) {
        return;
      }
      if (!confirm(`Delete recipe "${state.selectedRecipe}" and all of its shots?`)) {
        return;
      }
      try {
        const data = await api("/api/recipes/delete", {
          method: "POST",
          headers: { "Content-Type": "application/x-www-form-urlencoded" },
          body: params({ recipe: state.selectedRecipe, target_time: els.targetTime.value })
        });
        render(data);
        showToast("Recipe deleted");
      } catch (error) {
        showToast(error.message, true);
      }
    });

    els.sampleList.addEventListener("click", async event => {
      const button = event.target.closest(".delete-sample");
      if (!button) {
        return;
      }
      const label = button.dataset.sampleLabel || "this shot";
      if (!confirm(`Delete shot ${label}?`)) {
        return;
      }
      try {
        const data = await api("/api/samples/delete", {
          method: "POST",
          headers: { "Content-Type": "application/x-www-form-urlencoded" },
          body: params({
            recipe: state.selectedRecipe,
            sample_index: button.dataset.sampleIndex,
            target_time: els.targetTime.value
          })
        });
        render(data);
        showToast("Shot deleted");
      } catch (error) {
        showToast(error.message, true);
      }
    });

    els.sampleForm.addEventListener("submit", async event => {
      event.preventDefault();
      if (els.sampleForm.classList.contains("is-model-choice") && !els.grind.value) {
        showToast("Choose Linear or Curve grind", true);
        return;
      }
      try {
        const body = params({
          recipe: state.selectedRecipe,
          time: els.shotTime.value,
          grind: els.grind.value,
          choked: els.chokedShot.checked ? "1" : "0",
          target_time: els.targetTime.value
        });
        const data = await api("/api/samples", {
          method: "POST",
          headers: { "Content-Type": "application/x-www-form-urlencoded" },
          body
        });
        event.target.reset();
        syncChokedShot();
        els.sampleForm.classList.remove("is-model-choice");
        els.grind.required = true;
        resetModelChoice();
        els.sampleDialog.close();
        render(data);
        showToast("Shot added");
      } catch (error) {
        showToast(error.message, true);
      }
    });

    els.recipeForm.addEventListener("submit", async event => {
      event.preventDefault();
      try {
        const form = new FormData(event.target);
        const data = await api("/api/recipes", {
          method: "POST",
          headers: { "Content-Type": "application/x-www-form-urlencoded" },
          body: params({ recipe: form.get("recipe"), dose: form.get("dose"), shot_weight: form.get("shot_weight") })
        });
        event.target.reset();
        els.recipeDialog.close();
        render(data);
        showToast("Recipe created");
      } catch (error) {
        showToast(error.message, true);
      }
    });

    loadState().catch(error => showToast(error.message, true));
  </script>
</body>
</html>
"###
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

    if header != HEADER.trim_end() {
        return Err(Box::new(AppError::new(format!(
            "Unexpected data file header: {header}"
        ))));
    }

    let mut data = Data::default();
    for line in lines {
        let fields: Vec<&str> = line.split('\t').collect();
        match fields.first().copied() {
            Some("recipe") => data.recipes.push(Recipe {
                name: get_field(&fields, 1).to_string(),
                dose_weight_g: get_field(&fields, 2).to_string(),
                shot_weight_g: get_field(&fields, 3).to_string(),
                samples: Vec::new(),
            }),
            Some("sample") => {
                let sample = Sample {
                    recipe: get_field(&fields, 1).to_string(),
                    time: get_field(&fields, 4).to_string(),
                    grind: get_field(&fields, 5).to_string(),
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
        output.push('\t');
        output.push_str(&recipe.shot_weight_g);
        output.push_str("\t\t\n");
        for sample in &recipe.samples {
            output.push_str("sample\t");
            output.push_str(&sample.recipe);
            output.push_str("\t\t\t");
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

fn is_choked_sample(sample: &Sample) -> bool {
    let shot_time = numeric_value(&sample.time);
    is_number(&shot_time) && parse_number(&shot_time) == 0.0
}

fn numeric_grind_points(recipe: &Recipe) -> Vec<(f64, f64)> {
    recipe
        .samples
        .iter()
        .filter_map(|sample| {
            let grind = numeric_value(&sample.grind);
            let shot_time = numeric_value(&sample.time);
            if is_number(&grind) && is_number(&shot_time) && parse_number(&shot_time) > 0.0 {
                Some((parse_number(&grind), parse_number(&shot_time)))
            } else {
                None
            }
        })
        .collect()
}

fn choked_grinds(recipe: &Recipe) -> Vec<f64> {
    recipe
        .samples
        .iter()
        .filter_map(|sample| {
            let grind = numeric_value(&sample.grind);
            (is_choked_sample(sample) && is_number(&grind)).then(|| parse_number(&grind))
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

fn grind_axis_bounds_with_markers(
    points: &[(f64, f64)],
    marker_grinds: &[f64],
    focus_grind: Option<f64>,
) -> (f64, f64) {
    let sample_min = points
        .iter()
        .map(|(grind, _)| *grind)
        .chain(marker_grinds.iter().copied().filter(|value| value.is_finite()))
        .chain(focus_grind.filter(|value| value.is_finite()))
        .fold(f64::INFINITY, f64::min);
    let sample_max = points
        .iter()
        .map(|(grind, _)| *grind)
        .chain(marker_grinds.iter().copied().filter(|value| value.is_finite()))
        .chain(focus_grind.filter(|value| value.is_finite()))
        .fold(f64::NEG_INFINITY, f64::max);

    if !sample_min.is_finite() || !sample_max.is_finite() {
        return (1.0, 40.0);
    }

    if sample_min == sample_max {
        return ((sample_min - 1.0).max(1.0), (sample_max + 1.0).min(40.0));
    }

    let padding = ((sample_max - sample_min) * 0.1).max(1.0);
    let mut min = (sample_min - padding).floor();
    let mut max = (sample_max + padding).ceil();
    if focus_grind.is_none_or(|value| value >= 1.0) {
        min = min.max(1.0);
    }
    if focus_grind.is_none_or(|value| value <= 40.0) {
        max = max.min(40.0);
    }

    (min, max)
}

fn shot_time_axis_bounds(target_seconds: f64) -> (f64, f64) {
    let mut min: f64 = 0.0;
    let mut max: f64 = 60.0;

    if target_seconds.is_finite() {
        if target_seconds < min {
            let padding = ((max - target_seconds) * 0.08).max(2.0);
            min = (target_seconds - padding).floor();
        } else if target_seconds > max {
            let padding = ((target_seconds - min) * 0.08).max(2.0);
            max = (target_seconds + padding).ceil();
        }
    }

    if min == max {
        (min - 1.0, max + 1.0)
    } else {
        (min, max)
    }
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
    choke_grinds: &[f64],
    target_seconds: f64,
    predicted_grind: f64,
    intercept: f64,
    slope: f64,
    model_r2: f64,
) -> String {
    let width = 960.0;
    let height = 760.0;
    let left = 74.0;
    let right = 14.0;
    let top = 10.0;
    let bottom = 34.0;
    let plot_width = width - left - right;
    let plot_height = height - top - bottom;
    let (x_min, x_max) = grind_axis_bounds_with_markers(points, choke_grinds, Some(predicted_grind));
    let (y_min, y_max) = shot_time_axis_bounds(target_seconds);
    let model_y_at_min = intercept + (slope * x_min);
    let model_y_at_max = intercept + (slope * x_max);
    let exponential_model = exponential_time_model(model_points);

    let x = |grind: f64| left + ((grind - x_min) / (x_max - x_min) * plot_width);
    let y = |seconds: f64| top + ((y_max - seconds) / (y_max - y_min) * plot_height);

    let line_x1 = x(x_min);
    let line_y1 = y(model_y_at_min);
    let line_x2 = x(x_max);
    let line_y2 = y(model_y_at_max);
    let target_y = y(target_seconds);
    let predicted_x = x(predicted_grind);
    let title = svg_escape(&recipe.name);
    let desc = svg_escape(&format!(
        "Dose {}g, shot {}g, time = {} + {} * grind, R2 {}.",
        recipe.dose_weight_g,
        recipe.shot_weight_g,
        fmt(intercept),
        fmt(slope),
        fmt(model_r2)
    ));

    let mut svg = String::new();
    svg.push_str(&format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{width:.0}" height="{height:.0}" viewBox="0 0 {width:.0} {height:.0}" role="img" aria-labelledby="title desc">
<title id="title">Shot time vs grind for {title}</title>
<desc id="desc">{desc}</desc>
<rect width="100%" height="100%" fill="#15181d"/>
<rect x="{left:.0}" y="{top:.0}" width="{plot_width:.0}" height="{plot_height:.0}" fill="#1b2027" stroke="#343a42"/>
<clipPath id="plot-area"><rect x="{left:.0}" y="{top:.0}" width="{plot_width:.0}" height="{plot_height:.0}"/></clipPath>
"##
    ));

    for tick in axis_ticks(x_min, x_max) {
        let tick_x = x(tick);
        svg.push_str(&format!(
            r##"<line x1="{tick_x:.2}" y1="{top:.2}" x2="{tick_x:.2}" y2="{:.2}" stroke="#243044"/>
<line x1="{tick_x:.2}" y1="{:.2}" x2="{tick_x:.2}" y2="{:.2}" stroke="#64748b"/>
<text x="{tick_x:.2}" y="{:.2}" text-anchor="middle" font-family="Arial, sans-serif" font-size="19" font-weight="800" fill="#dbe7f5">{}</text>
"##,
            top + plot_height,
            top + plot_height,
            top + plot_height + 6.0,
            top + plot_height + 27.0,
            axis_label(tick)
        ));
    }

    for index in 0..=4 {
        let tick = y_min + ((y_max - y_min) * index as f64 / 4.0);
        let tick_y = y(tick);
        svg.push_str(&format!(
            r##"<line x1="{left:.2}" y1="{tick_y:.2}" x2="{:.2}" y2="{tick_y:.2}" stroke="#243044"/>
<line x1="{:.2}" y1="{tick_y:.2}" x2="{left:.2}" y2="{tick_y:.2}" stroke="#64748b"/>
<text x="{:.2}" y="{:.2}" text-anchor="end" font-family="Arial, sans-serif" font-size="18" font-weight="800" fill="#dbe7f5">{}s</text>
"##,
            left + plot_width,
            left - 6.0,
            left - 8.0,
            tick_y + 6.0,
            fmt(tick)
        ));
    }

    svg.push_str(&format!(
        r##"<g clip-path="url(#plot-area)">
"##
    ));

    let choke_limit = choke_grinds
        .iter()
        .copied()
        .filter(|value| value.is_finite())
        .max_by(|left, right| left.total_cmp(right));

    if let Some(choke_limit) = choke_limit {
        let shade_x = x(choke_limit).clamp(left, left + plot_width);
        let shade_width = (shade_x - left).max(0.0);
        svg.push_str(&format!(
            r##"<rect x="{left:.2}" y="{top:.2}" width="{shade_width:.2}" height="{plot_height:.2}" fill="#ff4d6d" opacity="0.08"/>
"##
        ));
    }

    if let Some(choke_limit) = choke_limit {
        let choke_x = x(choke_limit);
        svg.push_str(&format!(
            r##"<line x1="{choke_x:.2}" y1="{top:.2}" x2="{choke_x:.2}" y2="{:.2}" stroke="#ff4d6d" stroke-width="3" stroke-dasharray="2 8" opacity="0.82"/>
"##,
            top + plot_height
        ));
    }

    svg.push_str(&format!(
        r##"<line x1="{left:.2}" y1="{target_y:.2}" x2="{:.2}" y2="{target_y:.2}" stroke="#a6e22e" stroke-width="2" stroke-dasharray="7 5"/>
"##,
        left + plot_width
    ));

    if let Some((log_intercept, log_slope)) = exponential_model {
        let curve_points = (0..=48)
            .filter_map(|index| {
                let grind = x_min + ((x_max - x_min) * index as f64 / 48.0);
                let seconds = (log_intercept + (log_slope * grind)).exp();
                seconds
                    .is_finite()
                    .then(|| format!("{:.2},{:.2}", x(grind), y(seconds)))
            })
            .collect::<Vec<_>>()
            .join(" ");
        if !curve_points.is_empty() {
            svg.push_str(&format!(
                r##"<polyline points="{curve_points}" fill="none" stroke="#fbbf24" stroke-width="3" stroke-linecap="round" stroke-linejoin="round" stroke-dasharray="3 9" opacity="0.9"/>
"##
            ));
        }
    }

    svg.push_str(&format!(
        r##"<line x1="{line_x1:.2}" y1="{line_y1:.2}" x2="{line_x2:.2}" y2="{line_y2:.2}" stroke="#60a5fa" stroke-width="3"/>
"##
    ));

    if predicted_grind.is_finite() {
        svg.push_str(&format!(
            r##"<line x1="{predicted_x:.2}" y1="{top:.2}" x2="{predicted_x:.2}" y2="{:.2}" stroke="#a6e22e" stroke-width="2" stroke-dasharray="7 5"/>
<circle cx="{predicted_x:.2}" cy="{target_y:.2}" r="6" fill="#a6e22e" stroke="#15181d" stroke-width="2"/>
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

    svg.push_str("</svg>\n");

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

fn exponential_time_model(points: &[(f64, f64)]) -> Option<(f64, f64)> {
    let log_points = points
        .iter()
        .filter_map(|(grind, seconds)| {
            (*seconds > 0.0 && seconds.is_finite() && grind.is_finite())
                .then(|| (*grind, seconds.ln()))
        })
        .collect::<Vec<_>>();

    theil_sen_model(&log_points)
}

fn exponential_predicted_grind(points: &[(f64, f64)], target_seconds: f64) -> Option<f64> {
    if target_seconds <= 0.0 || !target_seconds.is_finite() {
        return None;
    }

    let (log_intercept, log_slope) = exponential_time_model(points)?;
    let predicted = (target_seconds.ln() - log_intercept) / log_slope;

    predicted.is_finite().then_some(predicted)
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

        assert_eq!(grind_axis_bounds_with_markers(&points, &[], None), (11.0, 20.0));
    }

    #[test]
    fn grind_axis_includes_predicted_goal_marker() {
        let points = vec![(12.2, 40.0), (18.7, 25.0)];

        let (min, max) = grind_axis_bounds_with_markers(&points, &[], Some(-5.0));

        assert!(min < -5.0);
        assert!(max > 18.7);
    }

    #[test]
    fn shot_time_axis_includes_target_marker() {
        assert_eq!(shot_time_axis_bounds(30.0), (0.0, 60.0));

        let (_, high_max) = shot_time_axis_bounds(90.0);
        let (low_min, _) = shot_time_axis_bounds(-5.0);

        assert!(high_max > 90.0);
        assert!(low_min < -5.0);
    }

    #[test]
    fn theil_sen_fit_error_distinguishes_insufficient_data_from_fit_failure() {
        assert!(theil_sen_fit_error(&[(12.0, 30.0)]).is_none());

        let error = theil_sen_fit_error(&[(12.0, 30.0), (12.0, 35.0)]).expect("fit error");

        assert!(error.contains("same grind setting"));
    }

    #[test]
    fn exponential_model_predicts_target_grind() {
        let points = vec![(10.0, 10.0), (12.0, 20.0)];

        let predicted = exponential_predicted_grind(&points, 20.0).expect("curve prediction");

        assert_eq!(fmt(predicted), "12.00");
        assert!(exponential_predicted_grind(&points, 0.0).is_none());
    }

    #[test]
    fn choked_samples_are_markers_not_model_points() {
        let recipe = Recipe {
            name: "Test".to_string(),
            dose_weight_g: "18".to_string(),
            shot_weight_g: "36".to_string(),
            samples: vec![
                Sample {
                    recipe: "Test".to_string(),
                    time: "0s".to_string(),
                    grind: "8".to_string(),
                },
                Sample {
                    recipe: "Test".to_string(),
                    time: "28s".to_string(),
                    grind: "12".to_string(),
                },
            ],
        };

        assert_eq!(numeric_grind_points(&recipe), vec![(12.0, 28.0)]);
        assert_eq!(choked_grinds(&recipe), vec![8.0]);
    }

    #[test]
    fn graph_svg_contains_prediction_context() {
        let recipe = Recipe {
            name: "Test & Espresso".to_string(),
            dose_weight_g: "18".to_string(),
            shot_weight_g: "36".to_string(),
            samples: Vec::new(),
        };
        let points = vec![(16.0, 25.0), (20.0, 35.0)];
        let model_points = points.clone();

        let svg = render_graph_svg(
            &recipe,
            &points,
            &model_points,
            &[11.0],
            30.0,
            18.0,
            -15.0,
            2.5,
            1.0,
        );

        assert!(svg.contains("<svg"));
        assert!(svg.contains("Test &amp; Espresso"));
        assert!(svg.contains("stroke=\"#60a5fa\""));
        assert!(svg.contains("stroke=\"#fbbf24\""));
        assert!(svg.contains("stroke-dasharray=\"3 9\""));
        assert!(svg.contains("fill=\"#ff4d6d\""));
        assert!(svg.contains("stroke=\"#ff4d6d\""));
        assert!(!svg.contains("local Theil-Sen line"));
        assert!(svg.contains(">60.00s</text>"));
    }
}
