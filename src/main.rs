use {
    clap::{Parser, Subcommand},
    colored::{ColoredString, Colorize},
    console::pad_str,
    rand::{SeedableRng, rngs::SmallRng, seq::SliceRandom},
    std::{
        env,
        fs::{File, OpenOptions, read_dir},
        io::{BufReader, Read, Result, Seek, SeekFrom, Write},
        path::{Path, PathBuf},
    },
    stoatformat::{
        Outcome,
        shogi::core::{Color, PieceType, Square},
        stoatpack::Stoatpack,
    },
};

#[derive(Parser)]
#[command(name = "spk-tools")]
#[clap(
    name = env!("CARGO_PKG_NAME"),
    version = env!("CARGO_PKG_VERSION"),
    author = env!("CARGO_PKG_AUTHORS"),
    about = env!("CARGO_PKG_DESCRIPTION"),
)]
struct Cli {
    #[clap(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Count(CountArgs),
    Fix(CommonArgs),
    Shuffle(ShuffleArgs),
}

impl Command {
    fn common(&self) -> &CommonArgs {
        match self {
            Command::Count(args) => &args.common,
            Command::Fix(args) => args,
            Command::Shuffle(args) => &args.common,
        }
    }
}

#[derive(Parser, Debug)]
struct CommonArgs {
    #[arg(short, long)]
    recursive: bool,

    #[arg(required = true)]
    paths: Vec<PathBuf>,
}

#[derive(Parser, Debug)]
struct CountArgs {
    #[clap(flatten)]
    common: CommonArgs,

    #[arg(long, short)]
    quick: bool,

    #[arg(long, short, default_value_t = 25001)]
    eval_limit: i16,
}

#[derive(Parser, Debug)]
struct ShuffleArgs {
    #[clap(flatten)]
    common: CommonArgs,

    #[arg(long, short, default_value_t = 42)]
    seed: u64,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let command = &cli.command;
    let args = command.common();

    let mut paths = Vec::new();

    for path in args.paths.clone() {
        if path.is_file() {
            paths.push(path);
        } else if path.is_dir() {
            paths.extend(get_files(&path, args.recursive)?);
        } else {
            eprintln!("Invalid path: {}", path.display());
        }
    }

    paths = paths
        .into_iter()
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("spk"))
        .collect();

    println!("Checking {} files...", paths.len());

    let mut total_positions = 0;
    let mut black_win = 0;
    let mut white_win = 0;
    let mut draw = 0;
    let mut reverse = 0;
    let mut total_records = 0;
    let mut total_broken_records = 0;
    let mut total_trimmed_bytes = 0;
    let mut fixed_files = 0;
    let mut king_squares = [0u64; 81];

    for path in paths {
        match command {
            Command::Count(args) => {
                let (positions, black_wins, white_wins, draws, reverses) =
                    count(path, args.quick, args.eval_limit, &mut king_squares)?;
                total_positions += positions;
                black_win += black_wins;
                white_win += white_wins;
                draw += draws;
                reverse += reverses;
            }
            Command::Fix(_) => {
                let (records, broken_records, trimmed_bytes) = fix(path)?;
                total_records += records;
                total_broken_records += broken_records;
                total_trimmed_bytes += trimmed_bytes;

                if total_broken_records != 0 {
                    fixed_files += 1;
                }
            }
            Command::Shuffle(args) => {
                let (records, broken_records) = shuffle(path, args.seed)?;
                total_records += records;
                total_broken_records += broken_records;
            }
        }
    }

    println!("               Summary               ");
    println!("-------------------------------------");

    match command {
        Command::Count(args) => {
            let games = black_win + white_win + draw;

            println!("Total positions: {}", total_positions);
            println!("Total games    : {}", games);
            println!(
                "Black wins     : {: <8} ({:.2}%)",
                black_win,
                black_win as f64 / games as f64 * 100.0f64
            );
            println!(
                "White wins     : {: <8} ({:.2}%)",
                white_win,
                white_win as f64 / games as f64 * 100.0f64
            );
            println!(
                "Draws          : {: <8} ({:.2}%)",
                draw,
                draw as f64 / games as f64 * 100.0f64
            );
            println!(
                "Reverses       : {: <8} ({:.2}%)",
                reverse,
                reverse as f64 / games as f64 * 100.0f64
            );

            if !args.quick {
                print_king_squares(total_positions, &king_squares);
            }
        }
        _ => {
            println!("Total records: {}", total_records);
            println!("Total broken records: {}", total_broken_records);
            println!("Total trimmed bytes: {}", total_trimmed_bytes);
            println!("Fixed files: {}", fixed_files);
        }
    }

    Ok(())
}

fn get_files(dir: &Path, recursive: bool) -> Result<Vec<PathBuf>> {
    let mut result = Vec::new();

    for entry in read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();

        if path.is_file() {
            result.push(path);
        } else if path.is_dir() && recursive {
            result.extend(get_files(&path, recursive)?);
        }
    }

    Ok(result)
}

fn count(
    path: PathBuf,
    quick: bool,
    eval_limit: i16,
    king_squares: &mut [u64; 81],
) -> Result<(usize, usize, usize, usize, usize)> {
    let file = OpenOptions::new().read(true).open(&path)?;
    let mut reader = BufReader::new(&file);
    let len = file.metadata()?.len();
    let mut total_positions = 0;
    let mut black_wins = 0;
    let mut white_wins = 0;
    let mut draws = 0;
    let mut reverses = 0;

    while reader.stream_position()? < len {
        let game = Stoatpack::deserialise(&mut reader)?;
        let mut pos = game.startpos;

        match game.wdl {
            Outcome::SenteWin => black_wins += 1,
            Outcome::SenteLoss => white_wins += 1,
            Outcome::Draw => draws += 1,
        }

        total_positions += game
            .moves
            .iter()
            .filter(|(_, score)| score.abs() <= eval_limit)
            .count()
            + 1;

        if (game.wdl == Outcome::SenteWin
            && game
                .moves
                .iter()
                .filter(|(_, score)| *score <= -eval_limit)
                .count()
                > 0)
            || game.wdl == Outcome::SenteLoss
                && game
                    .moves
                    .iter()
                    .filter(|(_, score)| *score >= eval_limit)
                    .count()
                    > 0
        {
            reverses += 1;
        }

        if !quick {
            let king_square = relative_square(
                pos.stm(),
                pos.piece_bb(PieceType::KING.with_color(pos.stm()))
                    .lsb()
                    .unwrap(),
            );
            king_squares[king_square.idx()] += 1;

            for mv in game.moves {
                pos = pos.apply_move(mv.0);
                let king_square = relative_square(
                    pos.stm(),
                    pos.piece_bb(PieceType::KING.with_color(pos.stm()))
                        .lsb()
                        .unwrap(),
                );
                king_squares[king_square.idx()] += 1;
            }
        }
    }

    Ok((total_positions, black_wins, white_wins, draws, reverses))
}

fn fix(path: PathBuf) -> Result<(usize, usize, u64)> {
    let mut file = OpenOptions::new().read(true).write(true).open(&path)?;
    let len = file.metadata()?.len();
    let (buffer, broken_records) = get_buffer(&file)?;
    let records = buffer.len();
    let mut trimmed_bytes = 0;

    if broken_records == 0 {
        println!("  OK  : {}, {} records", path.display(), records);
    } else {
        let buffer = buffer.into_iter().flatten().collect();
        write_buffer(&mut file, &buffer)?;
        trimmed_bytes = len - file.metadata()?.len();

        println!(
            "Fixed : {}, {} records, {} broken records, {} bytes trimmed",
            path.display(),
            records,
            broken_records,
            trimmed_bytes
        );
    }

    Ok((records, broken_records, trimmed_bytes))
}

fn shuffle(path: PathBuf, seed: u64) -> Result<(usize, usize)> {
    let mut file = OpenOptions::new().read(true).write(true).open(&path)?;
    let (mut buffer, broken_records) = get_buffer(&file)?;
    let records = buffer.len();

    if broken_records == 0 {
        let mut rng = SmallRng::seed_from_u64(seed);
        buffer.shuffle(&mut rng);
        write_buffer(&mut file, &buffer.into_iter().flatten().collect())?;
    } else {
        println!(
            "Shuffling is skipped because {} broken records",
            broken_records
        );
    }

    Ok((records, broken_records))
}

fn get_buffer(file: &File) -> Result<(Vec<Vec<u8>>, usize)> {
    let mut reader = BufReader::new(file);
    let len = file.metadata()?.len();
    let mut buffer = Vec::new();
    let mut broken_records = 0;
    let mut prev_pos = 0;

    while reader.stream_position()? < len {
        match Stoatpack::deserialise(&mut reader) {
            Ok(_) => {
                let curr_pos = reader.stream_position()?;
                let size = (curr_pos - prev_pos) as usize;
                let mut game_buffer = vec![0u8; size];

                reader.seek(SeekFrom::Start(prev_pos))?;
                reader.read_exact(&mut game_buffer)?;
                buffer.push(game_buffer);
                reader.seek(SeekFrom::Start(curr_pos))?;

                prev_pos = curr_pos;
            }
            Err(_) => {
                broken_records += 1;
            }
        }
    }

    Ok((buffer, broken_records))
}

fn write_buffer(file: &mut File, buffer: &Vec<u8>) -> Result<()> {
    file.seek(SeekFrom::Start(0))?;
    file.set_len(0)?;
    file.write_all(&buffer)?;

    Ok(())
}

fn relative_square(color: Color, square: Square) -> Square {
    if color == Color::SENTE {
        square
    } else {
        square.rotate()
    }
}

fn print_king_squares(total_positions: usize, king_squares: &[u64; 81]) {
    println!("King squares:");

    let border = "-".repeat(127);
    let space = format!("|{}", format!("{}|", " ".repeat(13)).repeat(9));
    println!("{}\n{}", border, space);

    let mut i = 0;
    let mut j = 9;
    let mut num = true;

    loop {
        let idx = if num { i } else { i + 1 - j };
        let square = (8 - idx / 9) * 9 + idx % 9;
        let text = if num {
            format!("{}", king_squares[square])
        } else {
            format!(
                "{}",
                colorize_ratio(king_squares[square] as f64 / total_positions as f64 * 100.0f64)
            )
        };

        print!(
            "| {} ",
            pad_str(&text, 11, console::Alignment::Center, Some(" "))
        );

        if !num {
            j -= 1;
        }

        if idx % 9 == 8 {
            num = !num;
            println!("|");

            if num {
                println!("{}\n{}", space, border);
            }

            if num && i != 80 {
                println!("{}", space)
            }

            if !num {
                j = 9;
            }
        }

        if num {
            i += 1;

            if i == 81 {
                break;
            }
        }
    }
}

fn colorize_ratio(ratio: f64) -> ColoredString {
    if ratio > 10.0 {
        format!("{:.2}%", ratio).red()
    } else if ratio > 5.0 {
        format!("{:.2}%", ratio).yellow()
    } else {
        format!("{:.2}%", ratio).blue()
    }
}
