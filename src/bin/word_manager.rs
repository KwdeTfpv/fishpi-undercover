use clap::{App, Arg, SubCommand};
use fishpi_undercover::word_bank::{Difficulty, WordBank, WordPair};

fn main() {
    let matches = App::new("词语管理器")
        .version("1.0")
        .about("管理卧底游戏词库")
        .subcommand(SubCommand::with_name("list").about("列出所有分类和词语"))
        .subcommand(
            SubCommand::with_name("add")
                .about("添加新词对")
                .arg(
                    Arg::with_name("category")
                        .help("分类名称")
                        .required(true)
                        .index(1),
                )
                .arg(
                    Arg::with_name("civilian")
                        .help("平民词语")
                        .required(true)
                        .index(2),
                )
                .arg(
                    Arg::with_name("undercover")
                        .help("卧底词语")
                        .required(true)
                        .index(3),
                )
                .arg(
                    Arg::with_name("similarity")
                        .help("相似度 (0.0-1.0)")
                        .required(true)
                        .index(4),
                )
                .arg(
                    Arg::with_name("difficulty")
                        .help("难度 (easy/medium/hard)")
                        .required(true)
                        .index(5),
                ),
        )
        .subcommand(SubCommand::with_name("stats").about("显示词库统计信息"))
        .subcommand(SubCommand::with_name("validate").about("验证词库完整性"))
        .subcommand(
            SubCommand::with_name("export").about("导出词库到文件").arg(
                Arg::with_name("file")
                    .help("输出文件路径")
                    .required(true)
                    .index(1),
            ),
        )
        .get_matches();

    // 初始化配置
    if let Err(e) = fishpi_undercover::config::Config::init() {
        eprintln!("配置初始化失败: {}", e);
        return;
    }

    let mut word_bank = WordBank::new();

    match matches.subcommand() {
        Some(("list", _)) => {
            println!("词库分类列表:");
            for category in word_bank.get_categories() {
                let count = word_bank.get_category_word_count(category);
                println!("  {}: {} 个词对", category, count);

                if let Some(words) = word_bank.get_category_words(category) {
                    for word in words {
                        println!(
                            "    {} - {} (相似度: {:.1}, 难度: {:?})",
                            word.civilian_word,
                            word.undercover_word,
                            word.similarity,
                            word.difficulty
                        );
                    }
                }
            }
        }
        Some(("add", args)) => {
            let category = args.value_of("category").unwrap();
            let civilian = args.value_of("civilian").unwrap();
            let undercover = args.value_of("undercover").unwrap();
            let similarity: f32 = args
                .value_of("similarity")
                .unwrap()
                .parse()
                .expect("相似度必须是数字");
            let difficulty = Difficulty::from_str(args.value_of("difficulty").unwrap());

            let word_pair = WordPair {
                civilian_word: civilian.to_string(),
                undercover_word: undercover.to_string(),
                similarity,
                difficulty,
            };

            word_bank.add_word_pair(category, word_pair);

            // 保存到文件
            if let Err(e) = word_bank.save_to_file(&word_bank.get_config().file_path) {
                eprintln!("保存词库失败: {}", e);
            } else {
                println!("成功添加词对: {} - {}", civilian, undercover);
            }
        }
        Some(("stats", _)) => {
            let stats = word_bank.get_stats();
            println!("词库统计信息:");
            println!("  总词对数: {}", stats.total_words);
            println!("  总分类数: {}", stats.total_categories);
            println!("  难度分布:");
            for (difficulty, count) in &stats.difficulty_stats {
                println!("    {:?}: {}", difficulty, count);
            }
            println!("  分类分布:");
            for (category, count) in &stats.category_stats {
                println!("    {}: {}", category, count);
            }
        }
        Some(("validate", _)) => {
            let errors = word_bank.validate();
            if errors.is_empty() {
                println!("词库验证通过！");
            } else {
                println!("词库验证发现 {} 个问题:", errors.len());
                for error in errors {
                    println!("  - {}", error);
                }
            }
        }
        Some(("export", args)) => {
            let file_path = args.value_of("file").unwrap();
            if let Err(e) = word_bank.save_to_file(file_path) {
                eprintln!("导出失败: {}", e);
            } else {
                println!("成功导出词库到: {}", file_path);
            }
        }
        _ => {
            println!("请使用 --help 查看可用命令");
        }
    }
}
