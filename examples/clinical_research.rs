//! Clinical Research Synthesis — multi-agent medical literature review with evidence scoring.
//!
//! Demonstrates MedResearch pack: claim extraction, citation verification,
//! evidence scoring, and cross-agent consensus auditing.
//!
//! Run: cargo run --example clinical_research

use prism_core::medresearch::{
    CitationVerifier, ClaimExtractor, ConsensusAuditor, EvidenceScorer,
};

fn main() {
    println!("=== Prism Clinical Research Synthesis ===\n");

    // Simulate 3 AI agents analyzing a clinical question
    let agent_outputs = vec![
        "Metformin reduced HbA1c by 1.5% compared to placebo in a randomized controlled trial \
         (n = 2,500 patients). The results were statistically significant (p < 0.001). \
         Treatment was well-tolerated with gastrointestinal side effects in 25% of patients. \
         Source: PMID: 28724542".to_string(),

        "In the UKPDS study, metformin reduced HbA1c by 1.5% and demonstrated a 32% reduction \
         in diabetes-related mortality. The sample size was n = 2,500 with p < 0.001. \
         Adverse events included gastrointestinal symptoms in approximately 25% of subjects. \
         Reference: 10.1016/S0140-6736(98)07019-6".to_string(),

        "Metformin therapy showed a 2.1% reduction in HbA1c levels in a meta-analysis of 12 trials. \
         The pooled effect was statistically significant with p < 0.05. \
         GI side effects were reported in 30% of the treatment group. \
         See: arXiv:2301.12345".to_string(),
    ];

    // Step 1: Extract claims from each agent
    println!("--- Step 1: Claim Extraction ---\n");
    for (i, output) in agent_outputs.iter().enumerate() {
        let claims = ClaimExtractor::extract(output);
        println!("Agent {} found {} claims:", i + 1, claims.len());
        for claim in &claims {
            println!("  [{:?}] {}", claim.claim_type,
                if claim.text.len() > 80 { format!("{}...", &claim.text[..80]) } else { claim.text.clone() });
        }
        println!();
    }

    // Step 2: Verify citations
    println!("--- Step 2: Citation Verification ---\n");
    let citations = ["PMID: 28724542", "10.1016/S0140-6736(98)07019-6", "arXiv:2301.12345", "some random text"];
    for citation in &citations {
        let status = CitationVerifier::verify(citation);
        println!("  {:>45} → {:?}", citation, status);
    }
    println!();

    // Step 3: Score evidence
    println!("--- Step 3: Evidence Scoring ---\n");
    let score1 = EvidenceScorer::score(
        "metformin reduces HbA1c",
        "Metformin reduced HbA1c by 1.5% in randomized controlled trial",
        "https://pubmed.ncbi.nlm.nih.gov/28724542",
        2024,
    );
    println!("  PubMed source (2024): relevance={:.2}, authority={:.2}, recency={:.2}, overall={:.2}",
        score1.relevance, score1.authority, score1.recency, score1.overall);

    let score2 = EvidenceScorer::score(
        "metformin reduces HbA1c",
        "Some blog post about diabetes medication",
        "https://health-blog.example.com/metformin",
        2019,
    );
    println!("  Blog source (2019):   relevance={:.2}, authority={:.2}, recency={:.2}, overall={:.2}",
        score2.relevance, score2.authority, score2.recency, score2.overall);
    println!();

    // Step 4: Consensus audit
    println!("--- Step 4: Cross-Agent Consensus Audit ---\n");
    let report = ConsensusAuditor::audit(&agent_outputs);
    println!("  Agents compared: {}", report.agents_compared);
    println!("  Agreement ratio: {:.0}%", report.agreement_ratio * 100.0);
    println!("  Agreed claims: {}", report.agreed_claims.len());
    println!("  Disputed claims: {}", report.disputed_claims.len());

    if !report.agreed_claims.is_empty() {
        println!("\n  Agreed (appear in 2+ agents):");
        for claim in report.agreed_claims.iter().take(3) {
            println!("    - {}", if claim.len() > 70 { format!("{}...", &claim[..70]) } else { claim.clone() });
        }
    }
    if !report.disputed_claims.is_empty() {
        println!("\n  Disputed (appear in only 1 agent):");
        for claim in report.disputed_claims.iter().take(3) {
            println!("    - {}", if claim.len() > 70 { format!("{}...", &claim[..70]) } else { claim.clone() });
        }
    }

    println!("\n=== Done ===");
}
