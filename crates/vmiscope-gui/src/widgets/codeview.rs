//! A line-numbered, syntax-tinted code panel.
//!
//! Used for generated scripts, MOF text and the WQL editor's read-only twin.
//! The lexer is deliberately small: this highlights to make structure readable
//! at a glance, not to parse. Being approximately right on every language beats
//! being exactly right on one, and a wrong colour is a much cheaper mistake
//! here than a dependency would be.

#![allow(dead_code)] // The views adopt the kit in the next commit.

use eframe::egui::text::LayoutJob;
use eframe::egui::{
    Color32, FontId, Frame, Label, Margin, RichText, ScrollArea, Stroke, TextFormat, TextStyle, Ui,
    Vec2,
};

use crate::theme::tokens::{a300, muted, DIVIDER, NEUTRAL, OK, R_MD, S2, S3, SURFACE, WARN};
use crate::widgets::button::accent_ramp;
use crate::widgets::rule::HAIRLINE;

/// Languages the tinter knows. Anything else renders as plain text, which is
/// the honest outcome for a language we have not taught it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Lang {
    Wql,
    PowerShell,
    CSharp,
    VbScript,
    Mof,
    Plain,
}

impl Lang {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Lang::Wql => "WQL",
            Lang::PowerShell => "PowerShell",
            Lang::CSharp => "C#",
            Lang::VbScript => "VBScript",
            Lang::Mof => "MOF",
            Lang::Plain => "Text",
        }
    }

    /// The comment introducer, or `None` where the language has none we detect.
    fn line_comment(self) -> Option<&'static str> {
        match self {
            Lang::Wql => Some("--"),
            Lang::PowerShell => Some("#"),
            Lang::CSharp | Lang::Mof => Some("//"),
            Lang::VbScript => Some("'"),
            Lang::Plain => None,
        }
    }

    fn keywords(self) -> &'static [&'static str] {
        match self {
            Lang::Wql => &[
                "SELECT",
                "FROM",
                "WHERE",
                "AND",
                "OR",
                "NOT",
                "LIKE",
                "IS",
                "NULL",
                "ISA",
                "WITHIN",
                "GROUP",
                "BY",
                "HAVING",
                "ORDER",
                "ASC",
                "DESC",
                "TRUE",
                "FALSE",
                "ASSOCIATORS",
                "REFERENCES",
                "OF",
            ],
            Lang::PowerShell => &[
                "param", "function", "foreach", "if", "else", "elseif", "return", "try", "catch",
                "finally", "begin", "process", "end", "in", "throw", "switch", "while", "do",
            ],
            Lang::CSharp => &[
                "using",
                "var",
                "new",
                "public",
                "private",
                "static",
                "void",
                "class",
                "foreach",
                "in",
                "if",
                "else",
                "return",
                "string",
                "int",
                "uint",
                "bool",
                "true",
                "false",
                "null",
                "namespace",
                "await",
                "async",
            ],
            Lang::VbScript => &[
                "Set",
                "Dim",
                "For",
                "Each",
                "Next",
                "If",
                "Then",
                "Else",
                "End",
                "Function",
                "Sub",
                "Wscript",
                "Echo",
                "GetObject",
                "In",
                "On",
                "Error",
                "Resume",
            ],
            Lang::Mof => &[
                "class",
                "instance",
                "of",
                "string",
                "uint8",
                "uint16",
                "uint32",
                "uint64",
                "sint32",
                "boolean",
                "datetime",
                "object",
                "ref",
                "implemented",
                "static",
                "key",
                "read",
                "write",
                "dynamic",
                "provider",
                "abstract",
            ],
            Lang::Plain => &[],
        }
    }
}

/// One coloured run of a line.
#[derive(Debug, PartialEq, Clone)]
pub(crate) struct Span {
    pub(crate) text: String,
    pub(crate) role: Role,
}

/// What a run means, resolved to a colour at paint time so the accent switch
/// reaches generated code too.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub(crate) enum Role {
    Plain,
    Keyword,
    Str,
    Comment,
    Number,
}

impl Role {
    fn color(self, ramp: &[Color32; 9]) -> Color32 {
        match self {
            Role::Plain => muted(88),
            Role::Keyword => a300(ramp),
            Role::Str => OK,
            Role::Comment => NEUTRAL[5],
            Role::Number => WARN,
        }
    }
}

/// Split one line into coloured runs.
///
/// Single pass, left to right, so a comment marker inside a string stays part
/// of the string and a quote inside a comment does not open one -- the two
/// mistakes that make naive highlighters look broken on real code.
pub(crate) fn tint_line(line: &str, lang: Lang) -> Vec<Span> {
    let mut spans: Vec<Span> = Vec::new();
    let mut buf = String::new();
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;

    let push = |spans: &mut Vec<Span>, buf: &mut String, lang: Lang| {
        if buf.is_empty() {
            return;
        }
        let role = classify_word(buf, lang);
        spans.push(Span {
            text: std::mem::take(buf),
            role,
        });
    };

    while i < chars.len() {
        let rest: String = chars[i..].iter().collect();

        if let Some(marker) = lang.line_comment() {
            if rest.starts_with(marker) {
                push(&mut spans, &mut buf, lang);
                spans.push(Span {
                    text: rest,
                    role: Role::Comment,
                });
                return spans;
            }
        }

        let c = chars[i];
        if c == '"' || c == '\'' {
            // VBScript uses ' for comments, so it never opens a string here --
            // the comment branch above already claimed it.
            push(&mut spans, &mut buf, lang);
            let quote = c;
            let mut s = String::from(quote);
            i += 1;
            while i < chars.len() {
                s.push(chars[i]);
                // A doubled quote is an escaped quote in WQL and VBScript, and
                // must not close the literal.
                if chars[i] == quote {
                    if chars.get(i + 1) == Some(&quote) {
                        s.push(quote);
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                i += 1;
            }
            // An unterminated literal runs to end of line rather than swallowing
            // the rest of the file.
            spans.push(Span {
                text: s,
                role: Role::Str,
            });
            continue;
        }

        if c.is_alphanumeric() || c == '_' || c == '.' {
            buf.push(c);
        } else {
            push(&mut spans, &mut buf, lang);
            spans.push(Span {
                text: c.to_string(),
                role: Role::Plain,
            });
        }
        i += 1;
    }
    push(&mut spans, &mut buf, lang);
    spans
}

fn classify_word(word: &str, lang: Lang) -> Role {
    if word.is_empty() {
        return Role::Plain;
    }
    if word.chars().all(|c| c.is_ascii_digit() || c == '.')
        && word.chars().any(|c| c.is_ascii_digit())
    {
        return Role::Number;
    }
    // WQL and MOF keywords are conventionally cased but not case-sensitive.
    let ci = matches!(
        lang,
        Lang::Wql | Lang::Mof | Lang::VbScript | Lang::PowerShell
    );
    for kw in lang.keywords() {
        let hit = if ci {
            kw.eq_ignore_ascii_case(word)
        } else {
            *kw == word
        };
        if hit {
            return Role::Keyword;
        }
    }
    Role::Plain
}

/// One line as a single laid-out run, with a section per coloured span.
///
/// It has to be one `LayoutJob` rather than a `Label` per span: egui inserts
/// `item_spacing.x` between widgets, so a span-per-label row renders
/// `SELECT * FROM x` as `SELECT  *  FROM  x` and destroys the one property a
/// monospace panel exists for. It is also one galley per line instead of one
/// per token.
fn line_job(font: &FontId, line: &str, lang: Lang, ramp: &[Color32; 9]) -> LayoutJob {
    let mut job = LayoutJob::default();
    for span in tint_line(line, lang) {
        job.append(
            &span.text,
            0.0,
            TextFormat {
                font_id: font.clone(),
                color: span.role.color(ramp),
                ..Default::default()
            },
        );
    }
    job
}

/// Render `code` in a surface panel with a line-number gutter.
pub(crate) fn code_panel(ui: &mut Ui, code: &str, lang: Lang) {
    let ramp = accent_ramp(ui);
    let lines: Vec<&str> = code.lines().collect();
    // Width the gutter by digit count so the code does not shift left when a
    // file crosses 100 lines.
    let digits = lines.len().max(1).to_string().len().max(2);
    let font = TextStyle::Monospace.resolve(ui.style());
    let gutter_w = ui.fonts_mut(|f| f.glyph_width(&font, '0')) * digits as f32;

    Frame::new()
        .fill(SURFACE)
        .corner_radius(R_MD)
        .stroke(Stroke::new(HAIRLINE, DIVIDER))
        .inner_margin(Margin::symmetric(S3 as i8, S2 as i8))
        .show(ui, |ui| {
            ScrollArea::both()
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(S2, 1.0);
                    for (n, line) in lines.iter().enumerate() {
                        ui.horizontal(|ui| {
                            ui.allocate_ui(Vec2::new(gutter_w, 0.0), |ui| {
                                ui.with_layout(
                                    eframe::egui::Layout::right_to_left(
                                        eframe::egui::Align::Center,
                                    ),
                                    |ui| {
                                        ui.add(Label::new(
                                            RichText::new((n + 1).to_string())
                                                .text_style(TextStyle::Monospace)
                                                .size(11.0)
                                                .color(muted(25)),
                                        ));
                                    },
                                );
                            });
                            ui.add(
                                Label::new(line_job(&font, line, lang, ramp))
                                    .selectable(true)
                                    .wrap_mode(eframe::egui::TextWrapMode::Extend),
                            );
                        });
                    }
                });
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roles(line: &str, lang: Lang) -> Vec<(String, Role)> {
        tint_line(line, lang)
            .into_iter()
            .map(|s| (s.text, s.role))
            .collect()
    }

    /// The reassembled spans must be the original line, or the panel silently
    /// drops characters -- the worst possible failure for a code view.
    #[test]
    fn tinting_never_loses_characters() {
        for (line, lang) in [
            (
                "SELECT Name FROM Win32_Process WHERE ProcessId = 42",
                Lang::Wql,
            ),
            (
                "$cim = New-CimSession -ComputerName 'DC01'  # connect",
                Lang::PowerShell,
            ),
            ("  [key, read] string Handle;", Lang::Mof),
            (
                "Set objWMI = GetObject(\"winmgmts:\") ' legacy",
                Lang::VbScript,
            ),
            ("", Lang::Plain),
        ] {
            let joined: String = tint_line(line, lang)
                .iter()
                .map(|s| s.text.as_str())
                .collect();
            assert_eq!(joined, line, "{lang:?} lost or invented characters");
        }
    }

    /// A comment marker inside a string is part of the string.
    #[test]
    fn a_comment_marker_inside_a_string_does_not_start_a_comment() {
        let spans = roles("WHERE Name = \"a # b\" AND x = 1", Lang::PowerShell);
        assert!(spans
            .iter()
            .any(|(t, r)| t == "\"a # b\"" && *r == Role::Str));
        assert!(
            !spans.iter().any(|(_, r)| *r == Role::Comment),
            "the # inside the literal opened a comment"
        );
    }

    /// A quote inside a comment does not open a string, and the comment runs to
    /// the end of the line.
    #[test]
    fn a_quote_inside_a_comment_does_not_open_a_string() {
        let spans = roles("# it's fine \"really", Lang::PowerShell);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].1, Role::Comment);
    }

    /// An unterminated literal must stop at end of line.
    #[test]
    fn an_unterminated_string_ends_at_the_line() {
        let spans = roles("SELECT * FROM x WHERE n = \"oops", Lang::Wql);
        let last = spans.last().expect("spans");
        assert_eq!(last.1, Role::Str);
        assert_eq!(last.0, "\"oops");
    }

    /// A doubled quote is an escape, not a terminator.
    #[test]
    fn doubled_quotes_stay_inside_the_literal() {
        let spans = roles("WHERE Name = \"say \"\"hi\"\"\"", Lang::Wql);
        assert!(spans
            .iter()
            .any(|(t, r)| *r == Role::Str && t.contains("hi")));
    }

    /// WQL keywords are case-insensitive; C# keywords are not.
    #[test]
    fn keyword_casing_follows_the_language() {
        assert_eq!(classify_word("select", Lang::Wql), Role::Keyword);
        assert_eq!(classify_word("SELECT", Lang::Wql), Role::Keyword);
        assert_eq!(classify_word("Var", Lang::CSharp), Role::Plain);
        assert_eq!(classify_word("var", Lang::CSharp), Role::Keyword);
    }

    #[test]
    fn numbers_are_numbers_and_identifiers_are_not() {
        assert_eq!(classify_word("42", Lang::Wql), Role::Number);
        assert_eq!(classify_word("10737418240", Lang::Wql), Role::Number);
        assert_eq!(classify_word("Win32_Process", Lang::Wql), Role::Plain);
        assert_eq!(classify_word("...", Lang::Wql), Role::Plain);
    }
}
