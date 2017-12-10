#[macro_use]
extern crate alumina;
extern crate rand;
extern crate clap;
extern crate image;
extern crate ndarray;
#[macro_use]
extern crate serde_derive;
extern crate serde;
extern crate bincode;
extern crate xz2;
extern crate byteorder;


mod network;

use std::fs::*;
use std::path::Path;
use std::io::{stdout, Write, Read};
use std::cmp;
use std::iter;
use std::num::FpCategory;

use bincode::{serialize, deserialize, Infinite};
use clap::{Arg, App, SubCommand, AppSettings, ArgMatches};

use network::*;

use ndarray::{ArrayD, Zip, IxDyn, Si, Axis};
use byteorder::{LittleEndian, ByteOrder};
use xz2::read::{XzEncoder, XzDecoder};

use alumina::opt::adam::Adam;
use alumina::data::image_folder::{ImageFolder, image_to_data, data_to_image};
use alumina::data::{DataSet, DataStream, Cropping};
use alumina::graph::Result;
use alumina::opt::{Opt, UnboxedCallbacks, CallbackSignal, max_steps};

const L1NATURAL_PARAMS: &'static [u8] = include_bytes!("res/L1SplineNatural.rsr");
const L1ANIME_PARAMS: &'static [u8] = include_bytes!("res/L1SplineAnime.rsr");
const L1FACES_PARAMS: &'static [u8] = include_bytes!("res/L1SplineFaces.rsr");


fn main() {
	let app_m = App::new("Rusty SR")
	.version("v0.2.0")
	.author("J. Millard <millard.jn@gmail.com>")
	.about("A convolutional neural network trained to upscale images")
	.settings(&[AppSettings::SubcommandsNegateReqs, AppSettings::VersionlessSubcommands])
	.arg(Arg::with_name("INPUT_FILE")
		.help("Sets the input image to upscale")
		.required(true)
		.index(1)
	)
	.arg(Arg::with_name("OUTPUT_FILE")
		.help("Sets the output file to write/overwrite (.png recommended)")
		.required(true)
		.index(2)
	)
	.arg(Arg::with_name("PARAMETERS")
		.help("Sets which built-in parameters to use with the neural net. Default: natural")
		.short("p")
		.long("parameters")
		.value_name("PARAMETERS")
		.possible_values(&["natural", "anime", "faces", "bilinear"])
		.empty_values(false)
	)
	.arg(Arg::with_name("CUSTOM")
		.conflicts_with("PARAMETERS")
		.short("c")
		.long("custom")
		.value_name("PARAMETER_FILE")
		.help("Sets a custom parameter file (.rsr) to use with the neural net")
		.empty_values(false)
	)
	.arg(Arg::with_name("BILINEAR_FACTOR")
		.short("f")
		.long("factor")
		.help("The integer upscaling factor used if bilinear upscaling is performed. Default 3")
		.empty_values(false)
	)
	.subcommand(SubCommand::with_name("train")
		.about("Train a new set of neural parameters on your own dataset")
		.arg(Arg::with_name("TRAINING_FOLDER")
			.required(true)
			.index(1)
			.help("Images from this folder(or sub-folders) will be used for training")
		)
		.arg(Arg::with_name("PARAMETER_FILE")
			.required(true)
			.index(2)
			.help("Learned network parameters will be (over)written to this parameter file (.rsr)")
		)
		.arg(Arg::with_name("LEARNING_RATE")
			.short("R")
			.long("rate")
			.help("The learning rate used by the Adam optimiser. Default: 1e-2")
			.empty_values(false)
		)
		.arg(Arg::with_name("QUANTISE")
			.short("q")
			.long("quantise")
			.help("Quantise the weights by zeroing the lowest 2 bytes of each. Reduces parameter file size")
			.takes_value(false)
		)
		.arg(Arg::with_name("TRAINING_LOSS")
	 		.help("Selects whether the neural net learns to minimise the L1 or L2 loss. Default: L1")
	 		.short("l")
	 		.long("loss")
	 		.value_name("LOSS")
	 		.possible_values(&["L1", "L2"])
	 		.empty_values(false)
	 	)
		.arg(Arg::with_name("DOWNSCALE_COLOURSPACE")
	 		.help("Colorspace in which to perform downsampling. Default: sRGB")
	 		.short("c")
	 		.long("colourspace")
	 		.value_name("COLOURSPACE")
	 		.possible_values(&["sRGB", "RGB"])
	 		.empty_values(false)
	 	)
		.arg(Arg::with_name("RECURSE_SUBFOLDERS")
			.short("r")
			.long("recurse")
			.help("Recurse into subfolders of training and validation folders looking for files")
			.takes_value(false)
		)
		.arg(Arg::with_name("START_PARAMETERS")
			.short("s")
			.long("start")
			.help("Start training from known parameters loaded from this .rsr file rather than random initialisation")
			.empty_values(false)
		)
		.arg(Arg::with_name("FACTOR")
			.short("f")
			.long("factor")
			.help("The integer upscaling factor of the network the be trained. Default: 3")
			.empty_values(false)
		)
		.arg(Arg::with_name("LOG_DEPTH")
			.short("d")
			.long("log_depth")
			.help("There will be 2^(log_depth)-1 hidden layers in the network the be trained. Default: 4")
			.empty_values(false)
		)
		.arg(Arg::with_name("VALIDATION_FOLDER")
			.short("v")
			.long("val_folder")
			.help("Images from this folder(or sub-folders) will be used to evaluate training progress")
			.empty_values(false)
		)
		.arg(Arg::with_name("VAL_MAX")
			.requires("VALIDATION_FOLDER")
			.short("m")
			.long("val_max")
			.value_name("N")
			.help("Set upper limit on number of images used for each validation pass")
			.empty_values(false)
		)
	)
	.subcommand(SubCommand::with_name("downsample")
	 	.about("Downsample images")
		.arg(Arg::with_name("FACTOR")
			.help("The integer downscaling factor")
			.required(true)
			.index(1)
		)
		.arg(Arg::with_name("INPUT_FILE")
			.help("Sets the input image to downscale")
			.required(true)
			.index(2)
		)
		.arg(Arg::with_name("OUTPUT_FILE")
			.help("Sets the output file to write/overwrite (.png recommended)")
			.required(true)
			.index(3)
		)
	 	.arg(Arg::with_name("COLOURSPACE")
	 		.help("colorspace in which to perform downsampling")
	 		.short("c")
	 		.long("colourspace")
	 		.value_name("COLOURSPACE")
	 		.possible_values(&["sRGB", "RGB"])
	 		.empty_values(false)
	 	)
	)
	.subcommand(SubCommand::with_name("psnr")
		.about("Print the PSNR value from the differences between the two images")
		.arg(Arg::with_name("IMAGE1")
			.required(true)
			.index(1)
			.help("PSNR is calculated using the difference between this image and IMAGE2")
		)
		.arg(Arg::with_name("IMAGE2")
			.required(true)
			.index(2)
			.help("PSNR is calculated using the difference between this image and IMAGE1")
		)
	)
	.get_matches();
	
	if let Some(sub_m) = app_m.subcommand_matches("train") {
		train(sub_m).unwrap();
	} if let Some(sub_m) = app_m.subcommand_matches("downsample") {
		downsample(sub_m).unwrap();
	} if let Some(sub_m) = app_m.subcommand_matches("psnr") {
		psnr(sub_m);
	} else {
		upscale(&app_m).unwrap();
	}
	
}


#[derive(Debug, Serialize, Deserialize)]
struct NetworkDescription {
	factor: usize,
	log_depth: u32,
	parameters: Vec<ArrayD<f32>>,
}


fn load_network(data: &[u8]) -> NetworkDescription {
	let decompressed = XzDecoder::new(data).bytes().collect::<::std::result::Result<Vec<_>, _>>().unwrap();
	let unshuffled = unshuffle(&decompressed, 4);
	let deserialized: NetworkDescription = deserialize(&unshuffled).expect("NetworkDescription decoding failed");
	deserialized
}

fn save_network(mut desc: NetworkDescription, quantize: bool) -> Vec<u8> {
	for arr in &mut desc.parameters {
		for e in arr.iter_mut() {
			if let FpCategory::Subnormal = e.classify(){
				*e = 0.0;
			}
			if quantize {
				let mut bytes = [0; 4];
				LittleEndian::write_f32(&mut bytes, *e);
				bytes[0] = 0;
				bytes[1] = 0;
				*e = LittleEndian::read_f32(&bytes);
			}
		}
	}
	
	let serialized: Vec<u8> = serialize(&desc, Infinite).expect("NetworkDescription encoding failed");
	let shuffled = shuffle(&serialized, 4);
	let compressed = XzEncoder::new(shuffled.as_slice(), 9).bytes().collect::<::std::result::Result<Vec<_>, _>>().unwrap();
	compressed
}

fn unshuffle(data: &[u8], stride: usize) -> Vec<u8> {
	let mut vec = vec![0; data.len()];
	let mut inc = 0;
	for offset in 0..stride {
		for i in 0..(data.len()-offset+stride-1)/stride {
			vec[offset + i*stride] = data[inc];
			inc += 1;
		}
	}
	debug_assert_eq!(inc, data.len());
	vec
}

fn shuffle(data: &[u8], stride: usize) -> Vec<u8> {
	let mut vec = Vec::with_capacity(data.len());
	for offset in 0..stride {
		for i in 0..(data.len()-offset+stride-1)/stride {
			vec.push(data[offset + i*stride])
		}
	}
	debug_assert_eq!(vec.len(), data.len());
	vec
}

fn downsample(app_m: &ArgMatches) -> Result<()>{

	let factor = match app_m.value_of("FACTOR") {
		Some(string) => string.parse::<usize>().expect("Factor argument must be an integer"),
		_ => unreachable!(),
	};

	let graph = match app_m.value_of("COLOURSPACE") {
		Some("RGB") | None => {
			print!("Downsampling using average pooling of linear RGB values...");
			downsample_lin_net(factor)?},
		Some("sRGB")=> {
			print!("Downsampling using average pooling of sRGB values...");
			downsample_srgb_net(factor)?},
		_ => unreachable!(),
	};

	stdout().flush().ok();

	let input_image = image::open(Path::new(app_m.value_of("INPUT_FILE").expect("No input file given?"))).expect("Error opening input image file.");

	let out_path = Path::new(app_m.value_of("OUTPUT_FILE").expect("No output file given?"));

	let input = image_to_data(&input_image);
	let shape = input.shape().to_vec();
	let input = input.into_shape(IxDyn(&[1, shape[0], shape[1], shape[2]])).unwrap();
	let input_id = graph.node_id("input")?.value_id();
	let output_id = graph.node_id("output")?.value_id();
	let mut subgraph = graph.subgraph(&[input_id.clone()], &[output_id.clone()])?;
	let result = subgraph.execute(vec![input]).expect("Could not execute downsampling graph");

	let output = result.get(&output_id).unwrap();

	print!(" Writing file...");
	stdout().flush().ok();
	data_to_image(output.subview(Axis(0), 0)).to_rgba().save(out_path).expect("Could not write output file");
	
	println!(" Done");
	Ok(())
}

fn upscale(app_m: &ArgMatches) -> Result<()>{
	let factor = match app_m.value_of("BILINEAR_FACTOR") {
		Some(string) => string.parse::<usize>().expect("Factor argument must be an integer"),
		_ => 3,
	};

	//-- Sort out parameters and graph
	let (params, graph) = if let Some(file_str) = app_m.value_of("CUSTOM") {
		let mut param_file = File::open(Path::new(file_str)).expect("Error opening parameter file");
		let mut data = Vec::new();
		param_file.read_to_end(&mut data).expect("Reading parameter file failed");
		print!("Upsampling using custom neural net parameters...");
		let network_desc = load_network(&data);
		(network_desc.parameters, inference_sr_net(network_desc.factor, network_desc.log_depth)?)
	} else {
		match app_m.value_of("PARAMETERS") {
			Some("natural") | None => {
				print!("Upsampling using natural neural net parameters...");
				let network_desc = load_network(L1NATURAL_PARAMS);
				(network_desc.parameters, inference_sr_net(network_desc.factor, network_desc.log_depth)?)},
			Some("anime")=> {
				print!("Upsampling using anime neural net parameters...");
				let network_desc = load_network(L1ANIME_PARAMS);
				(network_desc.parameters, inference_sr_net(network_desc.factor, network_desc.log_depth)?)},
			Some("faces")=> {
				print!("Upsampling using faces neural net parameters...");
				let network_desc = load_network(L1FACES_PARAMS);
				(network_desc.parameters, inference_sr_net(network_desc.factor, network_desc.log_depth)?)},
			Some("bilinear") => {
				print!("Upsampling using bilinear interpolation...");
				(Vec::new(), bilinear_net(factor)?)},
			_ => unreachable!(),
		}
	};
	stdout().flush().ok();

	let input_image = image::open(Path::new(app_m.value_of("INPUT_FILE").expect("No input file given?"))).expect("Error opening input image file.");

	let out_path = Path::new(app_m.value_of("OUTPUT_FILE").expect("No output file given?"));

	let input = image_to_data(&input_image);
	let shape = input.shape().to_vec();
	let input = input.into_shape(IxDyn(&[1, shape[0], shape[1], shape[2]])).unwrap();

	let mut input_vec = vec![input];
	input_vec.extend(params);
	let input_id = graph.node_id("input")?.value_id();
	let param_ids: Vec<_> = graph.parameter_ids().keys().map(|node_id| node_id.value_id()).collect();
	let mut subgraph_inputs = vec![input_id];
	subgraph_inputs.extend( param_ids);
	let output_id = graph.node_id("output")?.value_id();
	let mut subgraph = graph.subgraph(&subgraph_inputs, &[output_id.clone()])?;
	let result = subgraph.execute(input_vec).expect("Could not execute upsampling graph");

	let output = result.get(&output_id).unwrap();

	print!(" Writing file...");
	stdout().flush().ok();
	data_to_image(output.subview(Axis(0), 0)).to_rgba().save(out_path).expect("Could not write output file");
	
	println!(" Done");
	Ok(())
}


fn train(app_m: &ArgMatches) -> Result<()> {

	let srgb_downscale = match app_m.value_of("DOWNSCALE_COLORSPACE") {
		Some("sRGB") | None => true,
		Some("RGB") => false,
		_ => unreachable!(),
	};
	
	let (power, scale) = match app_m.value_of("TRAINING_LOSS") {
		Some("L1") | None => (1.0, 1.0/255.0), // L1, smoothed
		Some("L2") => (2.0, 1.0/255.0), // L2
		_ => unreachable!(),
	};

	let lr = match app_m.value_of("LEARNING_RATE") {
		None => 1e-2,
		Some(string) => string.parse::<f32>().expect("Learning rate argument must be a numeric value"),
	};

	let quantize = app_m.is_present("QUANTISE");

	let recurse = app_m.is_present("RECURSE_SUBFOLDERS");

	let training_folder = app_m.value_of("TRAINING_FOLDER").expect("No training folder?");

	let param_file_path = Path::new(app_m.value_of("PARAMETER_FILE").expect("No parameter file?")).to_path_buf();

	let mut factor_option = None;
	match app_m.value_of("FACTOR") {
		Some(string) => factor_option = Some(string.parse::<usize>().expect("Factor argument must be an integer")),
		_ => {},
	}

	let mut log_depth_option = None;
	match app_m.value_of("LOG_DEPTH") {
		Some(string) => log_depth_option = Some(string.parse::<u32>().expect("Log_depth argument must be an integer")),
		_ => {},
	}

	let mut params_option = None;
	if let Some(param_str) = app_m.value_of("START_PARAMETERS") {
		let mut param_file = File::open(Path::new(param_str)).expect("Error opening start parameter file");
		let mut data = Vec::new();
		param_file.read_to_end(&mut data).expect("Reading start parameter file failed");
		let network_desc = load_network(&data);
		if let Some(factor) = factor_option {
			if factor != network_desc.factor {
				println!("Using factor from parameter file ({}) rather than factor from argument ({})", network_desc.factor, factor);
			}
		}
		if let Some(log_depth) = log_depth_option {
			if log_depth != network_desc.log_depth {
				println!("Using log_depth from parameter file ({}) rather than log_depth from argument ({})", network_desc.log_depth, log_depth);
			}
		}
		params_option = Some(network_desc.parameters);
		factor_option = Some(network_desc.factor);
		log_depth_option = Some(network_desc.log_depth);
	}

	let factor = factor_option.unwrap_or(3);

	let log_depth = log_depth_option.unwrap_or(4);

	let graph = training_sr_net(factor, log_depth, 1e-6, power, scale, srgb_downscale)?;
	
	let batch_size = 4;
	let mut training_stream = ImageFolder::new(training_folder, recurse)
		.crop(0, &[48*factor, 48*factor, 3], Cropping::Random)
		.shuffle_random()
		.batch(batch_size)
		.buffered(16);

	let mut solver = Adam::new(&graph)?
		.rate(lr / batch_size as f32)
		.beta1(0.9)
		.beta2(0.99)
		.bias_correct(false);

	let params = params_option.unwrap_or_else(||{
		graph.initialise_nodes(solver.parameters()).expect("Could not initialise parameters")
	});

	let mut step_count = 0;
	solver.add_callback(move |data|{
		if step_count % 100 == 0 {
			let mut parameter_file = File::create(&param_file_path).expect("Could not make parameter file");
			let bytes = save_network(NetworkDescription{factor: factor, log_depth: log_depth, parameters: data.params.to_vec()}, quantize);
			parameter_file.write_all(&bytes).expect("Could not save to parameter file");
		}
		println!("step {}\terr:{}\tchange:{}", step_count, data.err/batch_size as f32, data.change_norm);
		step_count += 1;
		CallbackSignal::Continue
	});

	add_validation(app_m, &mut solver, recurse, factor, log_depth, srgb_downscale)?;

	solver.add_boxed_callback(max_steps(10_000_000));

	println!("Beginning Training");
	solver.optimise_from(&mut training_stream, params)?;	
	println!("Done");
	Ok(())
}

/// Add occasional test set evaluation as solver callback
fn add_validation(app_m: &ArgMatches, solver: &mut Opt,  recurse: bool, factor: usize, log_depth: u32, srgb_downscale: bool) -> Result<()>{
	if let Some(val_folder) = app_m.value_of("VALIDATION_FOLDER"){ 
		let power = 2.0;
		let scale = 1.0;
		let mut validation_graph = training_sr_net(factor, log_depth, 0.0, power, scale, srgb_downscale)?;

		let input_ids: Vec<_> = iter::once(&validation_graph.node_id("training_input")?).chain(solver.parameters()).map(|node_id| node_id.value_id()).collect();
		let output_id = validation_graph.node_id("output")?.value_id();
		let mut validation_subgraph = validation_graph.subgraph(&input_ids, &[output_id.clone()])?;

		let validation_set =ImageFolder::new(val_folder, recurse);
		let epoch_size = validation_set.length();
		let mut validation_stream = validation_set
			.shuffle_random()
			.batch(1)
			.buffered(8);

		let n: usize = app_m.value_of("VAL_MAX").map(|val_max|{
			cmp::min(epoch_size, val_max.parse::<usize>().expect("-val_max N must be a positive integer"))
		}).unwrap_or(epoch_size);

		let mut step_count = 0;
		solver.add_boxed_callback(Box::new(move |data|{

			if step_count % 100 == 0 {

				let mut err_sum = 0.0;
				let mut y_err_sum = 0.0;
				let mut pix_sum = 0.0f32;

				let mut psnr_sum = 0.0;
				let mut y_psnr_sum = 0.0;

				for _ in 0..n {
					let mut training_input = validation_stream.next();
					let training_input_copy = training_input[0].clone();
					training_input.extend(data.params.to_vec());

					let result = validation_subgraph.execute(training_input).expect("Could not execute upsampling graph");
					let output = result.get(&output_id).unwrap();

					let mut err = 0.0;
					let mut y_err = 0.0;
					let mut pix = 0.0f32;

					Zip::from(output.genrows())
						.and(training_input_copy.genrows())
						.apply(|o, i| {
							// let dr = (o[0].max(0.0).min(1.0)*255.0).round()/255.0 - i[0].max(0.0).min(1.0);
							// let dg = (o[1].max(0.0).min(1.0)*255.0).round()/255.0 - i[1].max(0.0).min(1.0);
							// let db = (o[2].max(0.0).min(1.0)*255.0).round()/255.0 - i[2].max(0.0).min(1.0);
							let dr = o[0].max(0.0).min(1.0) - i[0].max(0.0).min(1.0);
							let dg = o[1].max(0.0).min(1.0) - i[1].max(0.0).min(1.0);
							let db = o[2].max(0.0).min(1.0) - i[2].max(0.0).min(1.0);
							let y_diff = dr*0.299 + dg*0.587 + db*0.114; // 601
							//let y_diff = dr*0.2126 + dg*0.7152 + db*0.0722; // BT.709

							y_err += y_diff*y_diff;//(yo - yi)*(yo - yi);
							err += (dr*dr + dg*dg + db*db)/3.0; // R G B
							pix += 1.0;
						});

					psnr_sum += -10.0*(err/pix).log10();
					y_psnr_sum += -10.0*(y_err/pix).log10();

					pix_sum += pix;
					err_sum += err;
					y_err_sum += y_err;
				}

				psnr_sum /= n as f32;
				y_psnr_sum /= n as f32;
				let psnr = -10.0*(err_sum/pix_sum).log10();
				let y_psnr = -10.0*(y_err_sum/pix_sum).log10();
				println!("Validation PixAvgPSNR:\t{}\tPixAvgY_PSNR:\t{}\tImgAvgPSNR:\t{}\tImgAvgY_PSNR:\t{}", psnr, y_psnr, psnr_sum, y_psnr_sum);
			}
			step_count += 1;
			CallbackSignal::Continue
		}));
	}
	Ok(())
}

fn psnr(app_m: &ArgMatches){
	let image1 = image::open(Path::new(app_m.value_of("IMAGE1").expect("No input file given?"))).expect("Error opening image1 file.");
	let image2 = image::open(Path::new(app_m.value_of("IMAGE2").expect("No input file given?"))).expect("Error opening image2 file.");

	let image1 = image_to_data(&image1);
	let image2 = image_to_data(&image2);

	if image1.shape() != image2.shape() {
		println!("Image shapes will be cropped to the areas which overlap");
	}

	let min_height = cmp::min(image1.shape()[0], image2.shape()[0]);
	let min_width = cmp::min(image1.shape()[1], image2.shape()[2]);

	let slice_arg: [Si; 3] = [Si(0, Some(min_height as isize), 1), Si(0, Some(min_width as isize), 1), Si(0, Some(3), 1)];
	let image1 = image1.slice(&slice_arg);
	let image2 = image2.slice(&slice_arg);

	let mut err = 0.0;
	let mut y_err = 0.0;
	let mut pix = 0.0f32;

	Zip::from(image1.genrows())
		.and(image2.genrows())
		.apply(|o, i| {
			let dr = o[0].max(0.0).min(1.0)-i[0].max(0.0).min(1.0);
			let dg = o[1].max(0.0).min(1.0)-i[1].max(0.0).min(1.0);
			let db = o[2].max(0.0).min(1.0)-i[2].max(0.0).min(1.0);
			let y_diff = dr*0.299 + dg*0.587 + db*0.114; // 601
			//let y_diff = dr*0.2126 + dg*0.7152 + db*0.0722; // BT.709

			y_err += y_diff*y_diff;//(yo - yi)*(yo - yi);
			err += (dr*dr + dg*dg + db*db)/3.0; // R G B
			pix += 1.0;
		});

	let psnr = -10.0*(err/pix).log10();
	let y_psnr = -10.0*(y_err/pix).log10();

	println!("sRGB PSNR: {}\tLuma PSNR:{}", psnr, y_psnr);
}
