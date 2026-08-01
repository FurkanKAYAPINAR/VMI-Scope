//! The application's destinations, and the rail order that groups them.
//!
//! This replaces the five-tab strip. The design mock has seven destinations and
//! no home for the three security views this project already ships, so they
//! become a group of their own rather than being appended as an afterthought.

use crate::theme::icons;

/// Everywhere you can be.
#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub(crate) enum View {
    Explorer,
    Query,
    Events,
    Process,
    Network,
    Persistence,
    Providers,
    Saved,
    Compare,
    Machines,
    Settings,
}

/// A visual break in the rail. Not a destination.
#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub(crate) enum Group {
    Explore,
    Security,
    Data,
    /// Pinned to the bottom, away from the rest.
    Config,
}

impl View {
    /// Rail order. `Process` leads the security group deliberately: process
    /// start and stop is the telemetry the other three get correlated against
    /// -- which process opened that socket, wrote that subscription, hosts that
    /// provider.
    pub(crate) const ALL: [Self; 11] = [
        Self::Explorer,
        Self::Query,
        Self::Events,
        Self::Process,
        Self::Network,
        Self::Persistence,
        Self::Providers,
        Self::Saved,
        Self::Compare,
        Self::Machines,
        Self::Settings,
    ];

    pub(crate) fn group(self) -> Group {
        match self {
            Self::Explorer | Self::Query | Self::Events => Group::Explore,
            Self::Process | Self::Network | Self::Persistence | Self::Providers => Group::Security,
            Self::Saved | Self::Compare | Self::Machines => Group::Data,
            Self::Settings => Group::Config,
        }
    }

    pub(crate) fn icon(self) -> &'static str {
        match self {
            Self::Explorer => icons::TREE_STRUCTURE,
            Self::Query => icons::TERMINAL_WINDOW,
            Self::Events => icons::BROADCAST,
            Self::Process => icons::CPU,
            Self::Network => icons::GLOBE_HEMISPHERE_WEST,
            Self::Persistence => icons::SHIELD_WARNING,
            Self::Providers => icons::PLUGS_CONNECTED,
            Self::Saved => icons::BOOKMARK_SIMPLE,
            Self::Compare => icons::GIT_DIFF,
            Self::Machines => icons::HARD_DRIVES,
            Self::Settings => icons::GEAR_SIX,
        }
    }

    /// The 9px rail label. Deliberately shorter than [`View::title`] where the
    /// full word does not fit 56px of usable width -- "Persistence" at 9px runs
    /// past it, so the rail says "Persist" while every other surface, including
    /// the palette and the status bar, says the whole word.
    pub(crate) fn rail_label(self) -> &'static str {
        match self {
            Self::Persistence => "Persist",
            other => other.title(),
        }
    }

    /// The view's name, everywhere except the rail.
    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::Explorer => "Explorer",
            Self::Query => "Query",
            Self::Events => "Events",
            Self::Process => "Process",
            Self::Network => "Network",
            Self::Persistence => "Persistence",
            Self::Providers => "Providers",
            Self::Saved => "Saved",
            Self::Compare => "Compare",
            Self::Machines => "Machines",
            Self::Settings => "Settings",
        }
    }

    /// One line describing what the view is for. Used by the command palette,
    /// where a name alone does not tell you whether "Compare" means two hosts
    /// or two snapshots.
    pub(crate) fn hint(self) -> &'static str {
        match self {
            Self::Explorer => "Browse namespaces, classes and instances",
            Self::Query => "Run WQL against any namespace",
            Self::Events => "Watch a live WMI notification query",
            Self::Process => "Live process starts and stops",
            Self::Network => "Live TCP and UDP connections",
            Self::Persistence => "Hunt WMI event-subscription persistence",
            Self::Providers => "WMI providers and their host processes",
            Self::Saved => "Your query library",
            Self::Compare => "Diff two machines",
            Self::Machines => "Connection targets",
            Self::Settings => "Preferences",
        }
    }

    /// Views that are not built yet, so the rail can show them as coming
    /// without pretending they work. Removed as each one lands.
    pub(crate) fn is_placeholder(self) -> bool {
        matches!(
            self,
            Self::Query | Self::Process | Self::Saved | Self::Compare | Self::Machines
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rail is drawn from `ALL`, so a destination missing from it is
    /// unreachable however well its view is written.
    #[test]
    fn every_view_is_in_the_rail() {
        for view in [
            View::Explorer,
            View::Query,
            View::Events,
            View::Process,
            View::Network,
            View::Persistence,
            View::Providers,
            View::Saved,
            View::Compare,
            View::Machines,
            View::Settings,
        ] {
            assert!(View::ALL.contains(&view), "{view:?} is not in the rail");
        }
    }

    /// Grouping drives the rail's dividers; an ungrouped view would silently
    /// land in whichever group happened to be open.
    #[test]
    fn the_rail_is_ordered_by_group() {
        let order = [Group::Explore, Group::Security, Group::Data, Group::Config];
        let mut seen = 0;
        for view in View::ALL {
            let at = order
                .iter()
                .position(|g| *g == view.group())
                .expect("view has a known group");
            assert!(
                at >= seen,
                "{view:?} ({:?}) appears after a later group",
                view.group()
            );
            seen = at;
        }
    }

    /// Two views sharing an icon makes the rail ambiguous at 17px.
    #[test]
    fn icons_are_distinct() {
        let mut seen = std::collections::HashMap::new();
        for view in View::ALL {
            if let Some(prev) = seen.insert(view.icon(), view) {
                panic!("{view:?} and {prev:?} share an icon");
            }
        }
    }

    /// The rail has 56px of usable width; anything past about eight characters
    /// at 9px is clipped, which is why `rail_label` exists at all.
    #[test]
    fn rail_labels_fit() {
        for view in View::ALL {
            assert!(
                view.rail_label().len() <= 9,
                "{view:?} rail label {:?} is too long",
                view.rail_label()
            );
        }
    }
}
