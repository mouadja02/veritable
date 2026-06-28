use clap::ValueEnum;

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum Format {
    Human,
    Summary,
    Json,
    Jsonl,
}

impl std::fmt::Display for Format {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Format::Human => "human",
            Format::Summary => "summary",
            Format::Json => "json",
            Format::Jsonl => "jsonl",
        };
        write!(f, "{s}")
    }
}
