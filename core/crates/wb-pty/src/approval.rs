//! Is the terminal screen waiting on an approval prompt? Port of `src/shared/approvalDetect.ts`, same heuristic and
//! the same ceiling (a CLI restyling its menu breaks it). scripts/check-approval-parity.mts runs both on one corpus.
use std::sync::OnceLock;

use regex::Regex;

fn re(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("valid pattern"))
}

static BOX: OnceLock<Regex> = OnceLock::new();
static OPTION: OnceLock<Regex> = OnceLock::new();
static FOOTER: OnceLock<Regex> = OnceLock::new();
static YN: OnceLock<Regex> = OnceLock::new();
static YN_WORDS: OnceLock<Regex> = OnceLock::new();
static FENCE: OnceLock<Regex> = OnceLock::new();
static PROMPT_START: OnceLock<Regex> = OnceLock::new();
static NUMBERED: OnceLock<Regex> = OnceLock::new();

fn trim_end_js(s: &str) -> &str {
    s.trim_end()
}

/// The JS pattern is `^[❯›>]\s*(?!\d[.)]\s)`: an input line, unless what follows is a numbered option.
fn is_input_line(line: &str) -> bool {
    let Some(m) = re(&PROMPT_START, r"^[❯›>]\s*").find(line) else { return false };
    !re(&NUMBERED, r"^\d[.)]\s").is_match(&line[m.end()..])
}

pub fn looks_like_approval_prompt(screen: &str) -> bool {
    let box_chars = re(&BOX, "[│┃║╭╮╰╯┌┐└┘├┤─━═┏┓┗┛]");
    let option = re(&OPTION, r"^\s*([❯›>▶→]\s*)?(\d)[.)]\s+\S");
    let footer = re(&FOOTER, r"(?i)^(?:(?:press\s+)?(?:enter|esc(?:ape)?|tab|space)(?:\s+to\b|[/·])|use\s+(?:the\s+)?arrow\s+keys\b|[↑↓]+.*\bto\b)");
    let fence = re(&FENCE, r"^\s*(?:```|~~~)");
    let lines: Vec<String> = screen.split('\n').map(|l| trim_end_js(&box_chars.replace_all(l.trim_end_matches('\r'), " ")).to_string()).collect();
    let in_code = |end: usize| lines[..end].iter().filter(|l| fence.is_match(l)).count() % 2 == 1;

    // y/n has to be the real question on the last line, not a paragraph mentioning such a prompt.
    let tail = lines.iter().rev().find(|l| !l.trim().is_empty()).map(|l| l.trim().to_string()).unwrap_or_default();
    if !in_code(lines.len())
        && re(&YN, r"(?i)[(\[]\s*y\s*/\s*n\s*[)\]]\s*[?？:：]?\s*[yn]?$").is_match(&tail)
        && re(&YN_WORDS, r"(?i)[?？]|\b(?:overwrite|continue|proceed|confirm|allow|approve)\b|是否|確認").is_match(&tail)
    {
        return true;
    }

    let numbered: Vec<(usize, bool, String)> = lines
        .iter()
        .enumerate()
        .filter_map(|(i, l)| option.captures(l).map(|c| (i, c.get(1).is_some(), c[2].to_string())))
        .collect();
    let Some(last) = numbered.last() else { return false };
    let Some(start) = numbered.iter().rposition(|r| r.2 == "1") else { return false };
    let options = &numbered[start..];
    if options.len() < 2 || options.iter().enumerate().any(|(i, r)| r.2.parse::<usize>().unwrap_or(0) != i + 1) {
        return false;
    }
    // An example menu inside agent prose is not a prompt waiting for input.
    if in_code(options[0].0) {
        return false;
    }
    let after = &lines[last.0 + 1..];
    if !options.iter().any(|r| r.1) && !after.iter().any(|l| footer.is_match(l.trim())) {
        return false;
    }
    after.iter().all(|l| {
        let t = l.trim();
        t.is_empty() || footer.is_match(t) || (l.starts_with(char::is_whitespace) && !is_input_line(t) && !fence.is_match(l))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_prompts() {
        assert!(looks_like_approval_prompt("Do you want to proceed?\n❯ 1. Yes\n  2. No"));
        assert!(looks_like_approval_prompt("1. First result\n2. Second result\n\nOverwrite file? [y/N]"));
    }

    #[test]
    fn ordinary_output() {
        for s in [
            "I will approve the changes after the tests pass.",
            "1. Yes, the tests pass.\n2. No regressions were found.",
            "A confirmation prompt looks like this:\n```text\nAllow command?\n› 1. Yes\n  2. No\n```",
            "Do you want to proceed?\n❯ 1. Yes\n  2. No\n\n❯ ",
        ] {
            assert!(!looks_like_approval_prompt(s), "{s}");
        }
    }
}
