//! Everything wrong with a document, in one pass.
//!
//! ## Why not `Result`
//!
//! A terrain document is authored by hand and holds a hundred cross-references:
//! layers name sources, populations name recipes and modifier channels, material
//! layers name materials. When somebody renames a source, they break several
//! things at once — and a validator that returns `Result` tells them about the
//! first one, waits for a rebuild, then tells them about the second.
//!
//! That is not a small inconvenience. It sets the cost of a rename in
//! proportion to how many references it breaks, which is the opposite of what an
//! author needs: the whole point of validating is to find out what a change
//! costs *before* paying for it.
//!
//! So validation collects. It runs to the end and reports everything it found,
//! and only refuses to build if something was actually an error.
//!
//! ## Errors, warnings and notes
//!
//! Three severities, and the boundary between the first two is the only one that
//! carries weight: **an error means the document cannot be prepared**, a warning
//! means it can be but probably should not be. A layer naming a source that does
//! not exist is an error, because there is no sensible thing to sample. A layer
//! whose mask is empty everywhere is a warning, because it has an unambiguous
//! meaning — nothing — and the author may be halfway through something.
//!
//! Warnings that cannot be silenced become noise, and noise trains people to
//! ignore the errors printed beside it. So a warning here has to name a
//! condition somebody would actually want to fix.

use std::fmt;

/// How much a diagnostic matters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// Worth knowing, no action implied.
    Note,
    /// The document will prepare, and probably should not.
    Warning,
    /// The document cannot be prepared.
    Error,
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Note => "note",
            Self::Warning => "warning",
            Self::Error => "error",
        })
    }
}

/// Where in a document a diagnostic points.
///
/// A path rather than a line and column, because the document is read through a
/// deserialiser that has already thrown the file offsets away, and because a
/// path survives reformatting. `layers[3].mask.source` is more useful than
/// `line 84` when the file has been through a formatter since the error was
/// introduced.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Location {
    /// Dotted and indexed path from the document root.
    pub path: String,
    /// The asset the path is in, when the document spans several files.
    pub asset: Option<String>,
}

impl Location {
    pub fn at(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            asset: None,
        }
    }

    pub fn in_asset(mut self, asset: impl Into<String>) -> Self {
        self.asset = Some(asset.into());
        self
    }
}

impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.asset {
            Some(asset) => write!(f, "{asset}:{}", self.path),
            None => f.write_str(&self.path),
        }
    }
}

/// One thing that is wrong.
#[derive(Clone, Debug, PartialEq)]
pub struct Diagnostic {
    pub severity: Severity,
    /// A stable, greppable identifier: `unknown_source`, `duplicate_key`.
    ///
    /// Stable so that a build can be configured to ignore a specific warning
    /// without matching on prose, and so that a message can be reworded without
    /// breaking anything that keyed on it.
    pub code: &'static str,
    pub message: String,
    pub location: Location,
    /// What to do about it, when there is something specific to say.
    ///
    /// Optional because a vague suggestion is worse than none. "Check your
    /// configuration" is noise; "did you mean `main_path`?" is worth the line.
    pub help: Option<String>,
}

impl Diagnostic {
    pub fn error(code: &'static str, location: Location, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            code,
            message: message.into(),
            location,
            help: None,
        }
    }

    pub fn warning(code: &'static str, location: Location, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            code,
            message: message.into(),
            location,
            help: None,
        }
    }

    pub fn note(code: &'static str, location: Location, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Note,
            code,
            message: message.into(),
            location,
            help: None,
        }
    }

    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}[{}] {}: {}",
            self.severity, self.code, self.location, self.message
        )?;
        if let Some(help) = &self.help {
            write!(f, "\n  help: {help}")?;
        }
        Ok(())
    }
}

/// Everything one validation pass found.
#[derive(Clone, Debug, Default)]
pub struct DiagnosticReport {
    entries: Vec<Diagnostic>,
}

impl DiagnosticReport {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, diagnostic: Diagnostic) -> &mut Self {
        self.entries.push(diagnostic);
        self
    }

    pub fn error(
        &mut self,
        code: &'static str,
        location: Location,
        message: impl Into<String>,
    ) -> &mut Self {
        self.push(Diagnostic::error(code, location, message))
    }

    pub fn warning(
        &mut self,
        code: &'static str,
        location: Location,
        message: impl Into<String>,
    ) -> &mut Self {
        self.push(Diagnostic::warning(code, location, message))
    }

    pub fn note(
        &mut self,
        code: &'static str,
        location: Location,
        message: impl Into<String>,
    ) -> &mut Self {
        self.push(Diagnostic::note(code, location, message))
    }

    /// Fold another report in, keeping order.
    pub fn absorb(&mut self, other: DiagnosticReport) -> &mut Self {
        self.entries.extend(other.entries);
        self
    }

    pub fn entries(&self) -> &[Diagnostic] {
        &self.entries
    }

    /// The diagnostic most recently pushed.
    ///
    /// For attaching a "did you mean" to something already reported, where
    /// working out the suggestion needs a second pass over the candidates and is
    /// not worth threading through the reporting call.
    pub fn last_mut(&mut self) -> Option<&mut Diagnostic> {
        self.entries.last_mut()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn has_errors(&self) -> bool {
        self.entries.iter().any(|e| e.severity == Severity::Error)
    }

    pub fn count(&self, severity: Severity) -> usize {
        self.entries
            .iter()
            .filter(|e| e.severity == severity)
            .count()
    }

    /// Everything at one severity.
    pub fn at(&self, severity: Severity) -> impl Iterator<Item = &Diagnostic> {
        self.entries.iter().filter(move |e| e.severity == severity)
    }

    /// Turn a report into a `Result`, at the last moment.
    ///
    /// Collecting is for finding problems; a caller still has to be told whether
    /// to proceed. Warnings survive into the `Ok` side, because they are worth
    /// printing and are not worth stopping for.
    pub fn into_result<T>(self, value: T) -> Result<(T, DiagnosticReport), DiagnosticReport> {
        if self.has_errors() {
            Err(self)
        } else {
            Ok((value, self))
        }
    }

    /// A one-line summary: `3 errors, 1 warning`.
    pub fn summary(&self) -> String {
        let errors = self.count(Severity::Error);
        let warnings = self.count(Severity::Warning);
        let notes = self.count(Severity::Note);
        let mut parts = Vec::new();
        for (count, singular) in [(errors, "error"), (warnings, "warning"), (notes, "note")] {
            if count > 0 {
                parts.push(format!(
                    "{count} {singular}{}",
                    if count == 1 { "" } else { "s" }
                ));
            }
        }
        if parts.is_empty() {
            "no problems".to_string()
        } else {
            parts.join(", ")
        }
    }
}

impl fmt::Display for DiagnosticReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for entry in &self.entries {
            writeln!(f, "{entry}")?;
        }
        write!(f, "{}", self.summary())
    }
}

impl std::error::Error for DiagnosticReport {}

impl FromIterator<Diagnostic> for DiagnosticReport {
    fn from_iter<I: IntoIterator<Item = Diagnostic>>(iter: I) -> Self {
        Self {
            entries: iter.into_iter().collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report() -> DiagnosticReport {
        let mut report = DiagnosticReport::new();
        report
            .error(
                "unknown_source",
                Location::at("layers[3].mask.source"),
                "no source named `main_pth`",
            )
            .warning(
                "empty_mask",
                Location::at("layers[1].mask"),
                "this mask is zero everywhere",
            )
            .error(
                "duplicate_key",
                Location::at("materials[2].key"),
                "`grass_lush` is already defined",
            );
        report
    }

    #[test]
    fn a_pass_reports_everything_it_found() {
        // The property the whole module exists for. One rename breaks several
        // references, and telling the author about them one rebuild at a time
        // makes a rename cost as much as the damage it did.
        let report = report();
        assert_eq!(report.entries().len(), 3);
        assert_eq!(report.count(Severity::Error), 2);
        assert_eq!(report.count(Severity::Warning), 1);
        assert!(report.has_errors());
    }

    #[test]
    fn diagnostics_keep_the_order_they_were_found_in() {
        // Reordering would make two runs over the same document produce
        // different output, which is the sort of thing that makes people stop
        // trusting a tool.
        let codes: Vec<&str> = report().entries().iter().map(|e| e.code).collect();
        assert_eq!(codes, ["unknown_source", "empty_mask", "duplicate_key"]);
    }

    #[test]
    fn warnings_alone_do_not_stop_a_document() {
        let mut report = DiagnosticReport::new();
        report.warning("empty_mask", Location::at("layers[0]"), "zero everywhere");
        assert!(!report.has_errors());
        let (value, carried) = report.into_result(42).expect("warnings are not fatal");
        assert_eq!(value, 42);
        assert_eq!(
            carried.count(Severity::Warning),
            1,
            "the warning was dropped"
        );
    }

    #[test]
    fn errors_stop_a_document_and_come_back_with_the_warnings() {
        let result = report().into_result(());
        let returned = result.expect_err("errors are fatal");
        assert_eq!(returned.entries().len(), 3, "the warning was dropped");
    }

    #[test]
    fn a_summary_counts_and_pluralises() {
        assert_eq!(report().summary(), "2 errors, 1 warning");
        assert_eq!(DiagnosticReport::new().summary(), "no problems");
        let mut one = DiagnosticReport::new();
        one.error("x", Location::default(), "y");
        assert_eq!(one.summary(), "1 error");
    }

    #[test]
    fn a_location_says_which_asset_when_there_is_more_than_one() {
        let bare = Location::at("layers[0].mask");
        assert_eq!(bare.to_string(), "layers[0].mask");
        assert_eq!(
            bare.in_asset("grass_lab.terrain.ron").to_string(),
            "grass_lab.terrain.ron:layers[0].mask"
        );
    }

    #[test]
    fn help_appears_only_when_there_is_something_to_say() {
        let bare = Diagnostic::error("unknown_source", Location::at("a"), "no source `main_pth`");
        assert!(!bare.to_string().contains("help"));
        let helped = bare.with_help("did you mean `main_path`?");
        assert!(
            helped
                .to_string()
                .contains("help: did you mean `main_path`?")
        );
    }

    #[test]
    fn reports_fold_together_without_losing_order() {
        let mut first = DiagnosticReport::new();
        first.error("a", Location::default(), "one");
        let mut second = DiagnosticReport::new();
        second.error("b", Location::default(), "two");
        first.absorb(second);
        let codes: Vec<&str> = first.entries().iter().map(|e| e.code).collect();
        assert_eq!(codes, ["a", "b"]);
    }

    #[test]
    fn severity_orders_from_least_to_most_serious() {
        // So a report can be sorted by severity when a caller wants the errors
        // first, without anyone having to remember which way round it goes.
        assert!(Severity::Note < Severity::Warning);
        assert!(Severity::Warning < Severity::Error);
    }
}
