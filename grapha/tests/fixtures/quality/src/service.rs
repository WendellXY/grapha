pub fn load() {
    crate::repository::fetch();
    crate::ui::render_banner();
}
