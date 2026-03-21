//! MedResearch domain pack: citation-grounded verification for medical/scientific AI agents.
//!
//! - [`EvidenceScorer`] — Score how well a citation supports a claim.
//! - [`CitationVerifier`] — Validate PubMed, DOI, and arxiv citation formats.
//! - [`ClaimExtractor`] — Extract quantitative/verifiable claims from text.
//! - [`ConsensusAuditor`] — Compare multiple agent outputs for factual agreement.

use std::collections::{HashMap, HashSet};

// ---------------------------------------------------------------------------
// EvidenceScorer
// ---------------------------------------------------------------------------

/// Score how well a citation supports a claim.
#[derive(Debug, Clone)]
pub struct EvidenceScore {
    /// Keyword overlap between claim and citation (0.0-1.0).
    pub relevance: f64,
    /// Recency score — newer is better (0.0-1.0).
    pub recency: f64,
    /// Source authority score (0.0-1.0).
    pub authority: f64,
    /// Weighted overall score.
    pub overall: f64,
}

/// Scores claim-citation pairs for evidence quality.
pub struct EvidenceScorer;

impl EvidenceScorer {
    /// Score how well a citation supports a claim.
    ///
    /// - `claim`: The statement to verify.
    /// - `citation_text`: The text of the cited source.
    /// - `source_url`: URL or identifier of the source.
    /// - `year`: Publication year (for recency scoring).
    pub fn score(claim: &str, citation_text: &str, source_url: &str, year: u32) -> EvidenceScore {
        let relevance = keyword_overlap(claim, citation_text);
        let recency = recency_score(year);
        let authority = authority_score(source_url);
        let overall = relevance * 0.40 + recency * 0.25 + authority * 0.35;
        EvidenceScore { relevance, recency, authority, overall }
    }
}

fn keyword_overlap(a: &str, b: &str) -> f64 {
    let a_words: HashSet<String> = a.to_lowercase().split_whitespace()
        .filter(|w| w.len() > 3)
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_string())
        .collect();
    let b_words: HashSet<String> = b.to_lowercase().split_whitespace()
        .filter(|w| w.len() > 3)
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_string())
        .collect();
    if a_words.is_empty() { return 0.0; }
    let overlap = a_words.intersection(&b_words).count();
    (overlap as f64 / a_words.len() as f64).min(1.0)
}

fn recency_score(year: u32) -> f64 {
    let current_year = 2026u32;
    let age = current_year.saturating_sub(year);
    match age {
        0..=1 => 1.0,
        2..=3 => 0.8,
        4..=5 => 0.6,
        6..=10 => 0.4,
        _ => 0.2,
    }
}

fn authority_score(url: &str) -> f64 {
    let lower = url.to_lowercase();
    if lower.contains(".gov") || lower.contains("nih.gov") || lower.contains("cdc.gov") { return 0.95; }
    if lower.contains("pubmed") || lower.contains("ncbi.nlm") { return 0.92; }
    if lower.contains("nature.com") || lower.contains("science.org") || lower.contains("lancet") { return 0.90; }
    if lower.contains("arxiv.org") { return 0.85; }
    if lower.contains(".edu") { return 0.80; }
    if lower.contains("who.int") { return 0.90; }
    if lower.contains("wikipedia") { return 0.50; }
    0.40
}

// ---------------------------------------------------------------------------
// CitationVerifier
// ---------------------------------------------------------------------------

/// Status of a citation verification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CitationStatus {
    /// Citation matches a known format.
    Valid { format: String },
    /// Citation doesn't match any known format.
    InvalidFormat,
    /// Citation looks fabricated (e.g., wrong digit count for PMID).
    Suspicious { reason: String },
}

/// Validates citation formats: PubMed, DOI, arxiv.
pub struct CitationVerifier;

impl CitationVerifier {
    /// Verify a citation string against known formats.
    pub fn verify(citation: &str) -> CitationStatus {
        let trimmed = citation.trim();

        // PubMed ID: PMID followed by 7-8 digits
        if let Some(pmid) = trimmed.strip_prefix("PMID:").or_else(|| trimmed.strip_prefix("PMID ")) {
            let digits: String = pmid.trim().chars().filter(|c| c.is_ascii_digit()).collect();
            return if digits.len() >= 7 && digits.len() <= 9 {
                CitationStatus::Valid { format: "PubMed".into() }
            } else {
                CitationStatus::Suspicious { reason: format!("PMID should be 7-9 digits, got {}", digits.len()) }
            };
        }

        // DOI: starts with 10.
        if trimmed.starts_with("10.") || trimmed.contains("doi.org/10.") {
            let doi_part = if let Some(idx) = trimmed.find("10.") {
                &trimmed[idx..]
            } else { trimmed };
            return if doi_part.contains('/') && doi_part.len() > 7 {
                CitationStatus::Valid { format: "DOI".into() }
            } else {
                CitationStatus::Suspicious { reason: "DOI format incomplete".into() }
            };
        }

        // arxiv: YYMM.NNNNN format
        if trimmed.contains("arxiv") || trimmed.contains("arXiv") {
            let has_id = trimmed.chars().any(|c| c.is_ascii_digit());
            return if has_id {
                CitationStatus::Valid { format: "arxiv".into() }
            } else {
                CitationStatus::Suspicious { reason: "arxiv reference without ID".into() }
            };
        }

        // RFC: RFC followed by digits
        if trimmed.starts_with("RFC") || trimmed.starts_with("rfc") {
            let digits: String = trimmed.chars().filter(|c| c.is_ascii_digit()).collect();
            return if !digits.is_empty() {
                CitationStatus::Valid { format: "RFC".into() }
            } else {
                CitationStatus::InvalidFormat
            };
        }

        CitationStatus::InvalidFormat
    }
}

// ---------------------------------------------------------------------------
// ClaimExtractor
// ---------------------------------------------------------------------------

/// A quantitative or verifiable claim extracted from text.
#[derive(Debug, Clone)]
pub struct ExtractedClaim {
    /// The claim text.
    pub text: String,
    /// Type of claim.
    pub claim_type: ClaimType,
    /// The source sentence containing the claim.
    pub source_sentence: String,
}

/// Types of extractable claims.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaimType {
    Percentage,
    Comparison,
    Statistic,
    Dosage,
    SampleSize,
    Duration,
}

/// Extracts quantitative/verifiable claims from agent output text.
pub struct ClaimExtractor;

impl ClaimExtractor {
    /// Extract all verifiable claims from text.
    pub fn extract(text: &str) -> Vec<ExtractedClaim> {
        let mut claims = Vec::new();

        for sentence in text.split(|c: char| c == '.' || c == '\n') {
            let s = sentence.trim();
            if s.len() < 10 { continue; }

            // Percentages: X%
            if s.contains('%') {
                claims.push(ExtractedClaim {
                    text: s.to_string(),
                    claim_type: ClaimType::Percentage,
                    source_sentence: s.to_string(),
                });
            }

            // Comparisons: X-fold, X times
            let lower = s.to_lowercase();
            if lower.contains("-fold") || lower.contains(" times ") || lower.contains("compared to") {
                claims.push(ExtractedClaim {
                    text: s.to_string(),
                    claim_type: ClaimType::Comparison,
                    source_sentence: s.to_string(),
                });
            }

            // Statistics: p < 0.05, p-value, CI
            if lower.contains("p <") || lower.contains("p-value") || lower.contains("confidence interval")
                || lower.contains("statistically significant") {
                claims.push(ExtractedClaim {
                    text: s.to_string(),
                    claim_type: ClaimType::Statistic,
                    source_sentence: s.to_string(),
                });
            }

            // Sample sizes: n = X, N = X
            if lower.contains("n =") || lower.contains("n=") || lower.contains("sample size") {
                claims.push(ExtractedClaim {
                    text: s.to_string(),
                    claim_type: ClaimType::SampleSize,
                    source_sentence: s.to_string(),
                });
            }

            // Dosages: Xmg, X mg
            if lower.contains("mg") || lower.contains("ml") || lower.contains("dose") {
                claims.push(ExtractedClaim {
                    text: s.to_string(),
                    claim_type: ClaimType::Dosage,
                    source_sentence: s.to_string(),
                });
            }
        }

        claims
    }
}

// ---------------------------------------------------------------------------
// ConsensusAuditor
// ---------------------------------------------------------------------------

/// Report from comparing multiple agent outputs.
#[derive(Debug, Clone)]
pub struct AuditReport {
    /// Claims that appear in 2+ agent outputs.
    pub agreed_claims: Vec<String>,
    /// Claims that appear in only 1 agent output.
    pub disputed_claims: Vec<String>,
    /// Agreement ratio (agreed / total unique claims).
    pub agreement_ratio: f64,
    /// Number of agent outputs compared.
    pub agents_compared: usize,
}

/// Compares multiple agent outputs for factual agreement.
pub struct ConsensusAuditor;

impl ConsensusAuditor {
    /// Audit N agent outputs for factual agreement.
    pub fn audit(outputs: &[String]) -> AuditReport {
        if outputs.is_empty() {
            return AuditReport {
                agreed_claims: Vec::new(),
                disputed_claims: Vec::new(),
                agreement_ratio: 0.0,
                agents_compared: 0,
            };
        }

        // Extract claims from each output
        let all_claims: Vec<Vec<ExtractedClaim>> = outputs.iter()
            .map(|o| ClaimExtractor::extract(o))
            .collect();

        // Normalize claim text for comparison
        let mut claim_sources: HashMap<String, usize> = HashMap::new();
        for agent_claims in &all_claims {
            // Deduplicate per agent
            let unique: HashSet<String> = agent_claims.iter()
                .map(|c| c.text.to_lowercase().trim().to_string())
                .collect();
            for claim in unique {
                *claim_sources.entry(claim).or_insert(0) += 1;
            }
        }

        let agreed: Vec<String> = claim_sources.iter()
            .filter(|(_, count)| **count >= 2)
            .map(|(claim, _)| claim.clone())
            .collect();
        let disputed: Vec<String> = claim_sources.iter()
            .filter(|(_, count)| **count == 1)
            .map(|(claim, _)| claim.clone())
            .collect();

        let total = agreed.len() + disputed.len();
        let ratio = if total > 0 { agreed.len() as f64 / total as f64 } else { 0.0 };

        AuditReport {
            agreed_claims: agreed,
            disputed_claims: disputed,
            agreement_ratio: ratio,
            agents_compared: outputs.len(),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_evidence_scorer() {
        let score = EvidenceScorer::score(
            "aspirin reduces heart attack risk",
            "Aspirin therapy significantly reduces the risk of myocardial infarction in high-risk patients",
            "https://pubmed.ncbi.nlm.nih.gov/12345678",
            2024,
        );
        assert!(score.relevance > 0.3);
        assert!(score.authority > 0.8);
        assert!(score.recency > 0.7);
        assert!(score.overall > 0.5);
    }

    #[test]
    fn test_evidence_scorer_low_authority() {
        let score = EvidenceScorer::score(
            "some claim",
            "some citation text about it",
            "https://random-blog.com/article",
            2020,
        );
        assert!(score.authority < 0.5);
    }

    #[test]
    fn test_citation_verifier_pubmed() {
        assert!(matches!(
            CitationVerifier::verify("PMID: 12345678"),
            CitationStatus::Valid { format } if format == "PubMed"
        ));
    }

    #[test]
    fn test_citation_verifier_doi() {
        assert!(matches!(
            CitationVerifier::verify("10.1038/nature12373"),
            CitationStatus::Valid { format } if format == "DOI"
        ));
    }

    #[test]
    fn test_citation_verifier_arxiv() {
        assert!(matches!(
            CitationVerifier::verify("arXiv:2301.07041"),
            CitationStatus::Valid { format } if format == "arxiv"
        ));
    }

    #[test]
    fn test_citation_verifier_invalid() {
        assert_eq!(CitationVerifier::verify("just some text"), CitationStatus::InvalidFormat);
    }

    #[test]
    fn test_citation_verifier_suspicious_pmid() {
        assert!(matches!(
            CitationVerifier::verify("PMID: 123"),
            CitationStatus::Suspicious { .. }
        ));
    }

    #[test]
    fn test_claim_extractor_percentage() {
        let claims = ClaimExtractor::extract("The treatment showed a 45% reduction in symptoms");
        assert!(!claims.is_empty());
        assert_eq!(claims[0].claim_type, ClaimType::Percentage);
    }

    #[test]
    fn test_claim_extractor_comparison() {
        let claims = ClaimExtractor::extract("Drug A was 3-fold more effective compared to placebo");
        assert!(claims.iter().any(|c| c.claim_type == ClaimType::Comparison));
    }

    #[test]
    fn test_claim_extractor_statistic() {
        let claims = ClaimExtractor::extract("Results were statistically significant with p < 0.001");
        assert!(claims.iter().any(|c| c.claim_type == ClaimType::Statistic));
    }

    #[test]
    fn test_claim_extractor_sample_size() {
        let claims = ClaimExtractor::extract("The study enrolled patients with sample size n = 500");
        assert!(claims.iter().any(|c| c.claim_type == ClaimType::SampleSize));
    }

    #[test]
    fn test_consensus_auditor_agreement() {
        // Same claim text must appear in 2+ outputs for agreement
        let outputs = vec![
            "Treatment showed 45% improvement in patient outcomes".to_string(),
            "Treatment showed 45% improvement in patient outcomes".to_string(),
            "No significant benefit was observed".to_string(),
        ];
        let report = ConsensusAuditor::audit(&outputs);
        assert_eq!(report.agents_compared, 3);
        assert!(!report.agreed_claims.is_empty(), "Identical sentences should agree");
        assert!(report.agreement_ratio > 0.0);
    }

    #[test]
    fn test_consensus_auditor_empty() {
        let report = ConsensusAuditor::audit(&[]);
        assert_eq!(report.agents_compared, 0);
        assert_eq!(report.agreement_ratio, 0.0);
    }
}
