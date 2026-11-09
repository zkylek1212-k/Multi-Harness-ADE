// Prints what the mirror sees for recorded bytes. scripts/check-mirror-parity.mjs compares it with @xterm/headless.
// usage: mirror_text <file> <cols> <rows>            one screen for raw bytes
//        mirror_text --events <file> <cols> <rows>   lines of "d <hex>" and "r <cols> <rows>"; one screen after each event,
//                                                    then the whole buffer and the serialized snapshot
use wb_pty::mirror::Mirror;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a[1] != "--events" {
        let bytes = std::fs::read(&a[1]).expect("read");
        let mut m = Mirror::new(a[2].parse().unwrap(), a[3].parse().unwrap(), 0);
        m.process(&bytes);
        print!("{}", m.rows().join("\n"));
        return;
    }
    let text = std::fs::read_to_string(&a[2]).expect("read");
    let mut m = Mirror::new(a[3].parse().unwrap(), a[4].parse().unwrap(), 5000);
    for line in text.lines() {
        let mut parts = line.split(' ');
        match parts.next() {
            Some("d") => {
                let hex = parts.next().unwrap_or("");
                let bytes: Vec<u8> = (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap()).collect();
                m.process(&bytes);
            }
            Some("r") => m.resize(parts.next().unwrap().parse().unwrap(), parts.next().unwrap().parse().unwrap()),
            _ => {}
        }
        println!("{}\u{1}", m.rows().join("\n"));
    }
    println!("{}\u{1}", m.all_rows().join("\n"));
    print!("{}", m.serialize(5000));
}
