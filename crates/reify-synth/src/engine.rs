//! Task-scoped deterministic orchestration.
use crate::{cluster, compiler, prune};
use reify_core::{
    ReifyError,
    domain::{AuthContext, CapturedFrame, CompiledTool, Target},
};
use std::{collections::BTreeSet, time::Instant};

/// Compiled task tools in rank order; the first is the primary recommendation.
#[derive(Debug)]
pub struct SynthOutput {
    /// Up to five observed, relevant endpoint tools.
    pub tools: Vec<CompiledTool>,
    /// Name of the highest-ranked tool, always present in `tools`.
    pub primary_tool_name: String,
    /// Wall-clock compilation duration; excluded from deterministic content.
    pub synthesis_duration_ms: u64,
}
/// Stateless compiler; no browser, network, or runtime is required.
#[derive(Debug, Default, Clone, Copy)]
pub struct SynthEngine;
impl SynthEngine {
    /// Construct a stateless compiler.
    pub fn new() -> Self {
        Self
    }
    /// Synthesize up to five tools. No matching captures is an execution error;
    /// one or two supported endpoints yield one or two tools instead of fabrication.
    pub fn synthesize(
        &self,
        target: &Target,
        frames: &[CapturedFrame],
        auth: &AuthContext,
    ) -> Result<SynthOutput, ReifyError> {
        let start = Instant::now();
        Target::new(target.url.as_str(), target.goal.clone())?;
        let scoped: Vec<_> = frames
            .iter()
            .filter(|f| {
                let (Some(host), Some(candidate)) = (target.url.host_str(), f.url.host_str())
                else {
                    return false;
                };
                (host == candidate
                    || (target.url.domain().is_some()
                        && candidate
                            .strip_suffix(host)
                            .is_some_and(|s| s.ends_with('.'))))
                    && (f.is_json_response() || f.is_graphql())
            })
            .collect();
        let clusters = cluster::cluster_refs(scoped);
        let selected = prune::select(&clusters, target.goal.as_deref());
        let mut tools = Vec::new();
        let mut names = BTreeSet::new();
        for cluster in selected {
            let base = prune::tool_name(cluster);
            let mut name = base.clone();
            let mut suffix = 2;
            while !names.insert(name.clone()) {
                name = format!("{base}_{suffix}");
                suffix += 1;
            }
            tools.push(compiler::compile(cluster, auth, name)?);
        }
        let primary_tool_name = tools
            .first()
            .ok_or_else(|| {
                ReifyError::ExecutionError(
                    "no relevant API endpoints were captured for the target goal".into(),
                )
            })?
            .definition
            .name
            .clone();
        Ok(SynthOutput {
            tools,
            primary_tool_name,
            synthesis_duration_ms: start.elapsed().as_millis().min(u64::MAX as u128) as u64,
        })
    }
}
