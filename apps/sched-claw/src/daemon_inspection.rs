use anyhow::{Context, Result};

use crate::daemon_projection::{
    DaemonInspectionTarget, expected_daemon_projections, find_expected_daemon_projection,
};
use crate::daemon_protocol::find_expected_daemon_capability;
use crate::display::{
    OutputStyle, render_daemon_capability_detail, render_daemon_projection_detail,
    render_daemon_projection_list,
};

#[must_use]
pub fn render_daemon_projection_catalog(style: OutputStyle) -> String {
    render_daemon_projection_list(&expected_daemon_projections(), style)
}

pub fn render_daemon_inspection_target(
    target: DaemonInspectionTarget,
    style: OutputStyle,
) -> Result<String> {
    match target {
        DaemonInspectionTarget::Projection(name) => {
            let projection = find_expected_daemon_projection(name)
                .with_context(|| format!("unknown daemon projection `{}`", name.as_str()))?;
            Ok(render_daemon_projection_detail(&projection, style))
        }
        DaemonInspectionTarget::Capability(name) => {
            let capability = find_expected_daemon_capability(name)
                .with_context(|| format!("unknown daemon capability `{}`", name.as_str()))?;
            Ok(render_daemon_capability_detail(&capability, style))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{render_daemon_inspection_target, render_daemon_projection_catalog};
    use crate::daemon_projection::{DaemonInspectionTarget, DaemonProjectionName};
    use crate::daemon_protocol::DaemonCapabilityName;
    use crate::display::OutputStyle;

    #[test]
    fn renders_projection_catalog_from_shared_helper() {
        let rendered = render_daemon_projection_catalog(OutputStyle::Plain);
        assert!(rendered.contains("Daemon Projections"));
        assert!(rendered.contains("collect-perf"));
    }

    #[test]
    fn renders_projection_target_from_shared_helper() {
        let rendered = render_daemon_inspection_target(
            DaemonInspectionTarget::Projection(DaemonProjectionName::Activate),
            OutputStyle::Plain,
        )
        .expect("projection target");
        assert!(rendered.contains("Daemon Projection · activate"));
    }

    #[test]
    fn renders_capability_target_from_shared_helper() {
        let rendered = render_daemon_inspection_target(
            DaemonInspectionTarget::Capability(DaemonCapabilityName::PerfRecordCapture),
            OutputStyle::Plain,
        )
        .expect("capability target");
        assert!(rendered.contains("Daemon Capability · perf_record_capture"));
    }
}
