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
    let Some(last) = numbered.last() else { return unnumbered_menu(&lines).is_some() };
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

/// A menu without numbers, like Claude's folder trust screen: a marked option, then only indented lines and a footer
/// that mentions Enter. Returns the marked line. The Enter requirement keeps "esc to interrupt" under an input line out.
/// ponytail: same restyling ceiling as above; a CLI input line followed by an "enter to send" hint would match.
fn unnumbered_menu(lines: &[String]) -> Option<usize> {
    static MARKED: OnceLock<Regex> = OnceLock::new();
    static ENTER: OnceLock<Regex> = OnceLock::new();
    let marked = re(&MARKED, r"^\s*[❯›>▶→]\s*\S");
    let footer = re(&FOOTER, r"(?i)^(?:(?:press\s+)?(?:enter|esc(?:ape)?|tab|space)(?:\s+to\b|[/·])|use\s+(?:the\s+)?arrow\s+keys\b|[↑↓]+.*\bto\b)");
    let fence = re(&FENCE, r"^\s*(?:```|~~~)");
    let m = lines.iter().rposition(|l| marked.is_match(l))?;
    if lines[..m].iter().filter(|l| fence.is_match(l)).count() % 2 == 1 {
        return None;
    }
    let after = &lines[m + 1..];
    let enter = re(&ENTER, r"(?i)\benter\b");
    let ok = after.iter().any(|l| footer.is_match(l.trim()) && enter.is_match(l))
        && after.iter().all(|l| {
            let t = l.trim();
            t.is_empty() || footer.is_match(t) || (l.starts_with(char::is_whitespace) && !is_input_line(t) && !fence.is_match(l))
        });
    ok.then_some(m)
}

/// Keys that answer a recognized prompt. Enter takes the highlighted option; Esc is "no / cancel" in all three CLIs (Phase 0, 2026-11-12).
pub const APPROVE_KEYS: &str = "\r";
pub const REJECT_KEYS: &str = "\x1b";

/// A prompt the backend can answer, or only show. Desktop and phone both get this from the backend and never parse screens themselves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalPrompt {
    pub title: String,
    pub options: Vec<String>,
    /// Index of the highlighted option, if a marker shows one
    pub selected: Option<usize>,
    /// Enter would pick option 1 and it is a "Yes"
    pub can_approve: bool,
    /// Some option is a "No", which Esc stands for
    pub can_reject: bool,
    /// Title plus options; checked again right before any key is sent
    pub fingerprint: String,
}

/// ponytail: the same heuristic ceiling as looks_like_approval_prompt; a restyled menu makes this return None, which only costs the buttons
pub fn parse_approval_prompt(screen: &str) -> Option<ApprovalPrompt> {
    if !looks_like_approval_prompt(screen) {
        return None;
    }
    static LABEL: OnceLock<Regex> = OnceLock::new();
    let label = re(&LABEL, r"^\s*([❯›>▶→]\s*)?(\d)[.)]\s+(.*\S)");
    let box_chars = re(&BOX, "[│┃║╭╮╰╯┌┐└┘├┤─━═┏┓┗┛]");
    let lines: Vec<String> = screen.split('\n').map(|l| box_chars.replace_all(l.trim_end_matches('\r'), " ").trim_end().to_string()).collect();
    let rows: Vec<(usize, bool, usize, String)> = lines
        .iter()
        .enumerate()
        .filter_map(|(i, l)| label.captures(l).map(|c| (i, c.get(1).is_some(), c[2].parse().unwrap_or(0), c[3].trim().to_string())))
        .collect();
    // No numbers: shown only. These are setup questions like folder trust, answered by the user in the terminal (DEC-013).
    if rows.is_empty() {
        let m = unnumbered_menu(&lines)?;
        let footer = re(&FOOTER, r"(?i)^(?:(?:press\s+)?(?:enter|esc(?:ape)?|tab|space)(?:\s+to\b|[/·])|use\s+(?:the\s+)?arrow\s+keys\b|[↑↓]+.*\bto\b)");
        let marker = |s: &str| s.trim().trim_start_matches(['❯', '›', '>', '▶', '→']).trim().to_string();
        let options: Vec<String> = std::iter::once(marker(&lines[m]))
            .chain(lines[m + 1..].iter().map(|l| l.trim()).take_while(|t| !t.is_empty() && !footer.is_match(t)).map(String::from))
            .collect();
        let above = || lines[..m].iter().rev().map(|l| l.trim()).filter(|l| !l.is_empty());
        let title = above().take(12).find(|l| l.contains('?')).or_else(|| above().next()).unwrap_or("").to_string();
        let fingerprint = format!("{title}\n{}", options.join("\n"));
        return Some(ApprovalPrompt { title, options, selected: Some(0), can_approve: false, can_reject: false, fingerprint });
    }
    // The detector already proved the last run from 1 is consecutive
    let start = rows.iter().rposition(|r| r.2 == 1)?;
    let menu = &rows[start..];
    let options: Vec<String> = menu.iter().map(|r| r.3.clone()).collect();
    let selected = menu.iter().position(|r| r.1);
    let title = lines[..menu[0].0].iter().rev().map(|l| l.trim()).find(|l| !l.is_empty() && !label.is_match(l)).unwrap_or("").to_string();
    let yes = |s: &str| s.to_lowercase().starts_with("yes");
    let no = |s: &str| s.to_lowercase().starts_with("no");
    let can_approve = selected.unwrap_or(0) == 0 && options.first().is_some_and(|o| yes(o));
    let can_reject = options.iter().any(|o| no(o));
    let fingerprint = format!("{title}\n{}", options.join("\n"));
    Some(ApprovalPrompt { title, options, selected, can_approve, can_reject, fingerprint })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Screens recorded from the real CLIs in the Phase 0 spike (Claude 2.1.322, Codex 0.167.2, agy 1.5.6)
    #[test]
    fn parses_real_cli_prompts() {
        let claude = "● Write(a.txt)\n Do you want to create a.txt?\n ❯ 1. Yes\n   2. Yes, allow all edits during this session (shift+tab)\n   3. No\n\n Esc to cancel · Tab to amend";
        let p = parse_approval_prompt(claude).unwrap();
        assert_eq!(p.title, "Do you want to create a.txt?");
        assert_eq!((p.selected, p.can_approve, p.can_reject, p.options.len()), (Some(0), true, true, 3));

        let codex = "  Would you like to make the following edits?\n\n› 1. Yes, proceed (y)\n  2. Yes, and don't ask again for these files (a)\n  3. No, and tell Codex what to do differently (esc)\n\n  Press enter to confirm or esc to cancel";
        let p = parse_approval_prompt(codex).unwrap();
        assert_eq!(p.title, "Would you like to make the following edits?");
        assert!(p.can_approve && p.can_reject);

        let agy = "Allow creation of this file?\n> 1. Yes, allow creation\n  2. No, deny creation\n\n  ↑/↓ Navigate · enter Confirm";
        let p = parse_approval_prompt(agy).unwrap();
        assert!(p.can_approve && p.can_reject);
    }

    #[test]
    fn no_buttons_when_not_a_plain_yes_no() {
        // Codex's trust screen and its Ctrl+C menu: answering them is the user's call in the terminal
        let trust = "  Trust this folder?\n\n› 1. Trust and continue\n  2. Back to Agent Command Center\n\n  enter continue · esc back";
        let p = parse_approval_prompt(trust).unwrap();
        assert!(!p.can_approve && !p.can_reject);
        // Cursor moved off option 1: Enter would not approve
        let moved = "Run this command?\n  1. Yes, run command\n> 2. No, cancel\n\n  ↑/↓ Navigate · enter Confirm";
        let p = parse_approval_prompt(moved).unwrap();
        assert_eq!(p.selected, Some(1));
        assert!(!p.can_approve && p.can_reject);
        assert_eq!(parse_approval_prompt("I will approve the changes after the tests pass."), None);
    }

    #[test]
    fn claude_folder_trust_is_shown_not_answered() {
        // Recorded from Claude 2.1.322 starting in a fresh node worktree (2026-11-13)
        let trust = " Accessing workspace:\n C:\\tmp\\demo\\.cowork\\c\\n\n Quick safety check: Is this a project you created or one you trust? (Like your own code, a\n well-known open source project, or work from your team).\n Claude Code'll be able to read, edit, and execute files here.\n Security guide\n ❯ No, exit\n   Yes, I trust this folder\n\n Enter to confirm · Esc to cancel ";
        assert!(looks_like_approval_prompt(trust));
        let p = parse_approval_prompt(trust).unwrap();
        assert!(p.title.starts_with("Quick safety check"), "{}", p.title);
        assert_eq!(p.options, ["No, exit", "Yes, I trust this folder"]);
        assert!(!p.can_approve && !p.can_reject);
        // An input line with a busy hint under it is not a menu
        assert!(!looks_like_approval_prompt("❯ fix the tests\n  esc to interrupt"));
        assert!(!looks_like_approval_prompt("> quoted reply\n  more of it"));
    }

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
