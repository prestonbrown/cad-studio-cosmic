// Writes libcosmic default light + dark themes into cosmic-config, as cosmic-settings would on a real COSMIC desktop.
use cosmic_config::CosmicConfigEntry;
fn main() {
    for (t, cfg) in [
        (cosmic_theme::Theme::light_default(), cosmic_theme::Theme::light_config().unwrap()),
        (cosmic_theme::Theme::dark_default(), cosmic_theme::Theme::dark_config().unwrap()),
    ] {
        t.write_entry(&cfg).unwrap();
    }
    println!("seeded");
}
