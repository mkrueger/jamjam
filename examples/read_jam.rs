use std::env;

use jamjam::jam::JamMessageBase;

pub fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() != 2 {
        eprintln!("Usage: read_jam <filename>");
        std::process::exit(1);
    }

    let base = JamMessageBase::open(&args[1]).unwrap();

    for header in base.messages().flatten() {
        println!(
            "From:{:40}  Number:{}/{}",
            header.from().unwrap(),
            header.message_number - base.lowest_message_number() + 1,
            base.highest_message_number()
        );
        println!("From:{}", header.to().unwrap());
        println!("Subj:{}", header.subject().unwrap());
        println!("--------------------------------------------------");
        println!("{}", base.read_message_text(&header).unwrap());
    }
}
