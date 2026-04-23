/// Controls which optional fields appear in CLI output.
///
/// Fields can be selected via `--fields` flag (comma-separated) or configured
/// in `grapha.toml` under `[output] default_fields`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldSet {
    pub file: bool,
    pub id: bool,
    pub locator: bool,
    pub module: bool,
    pub repo: bool,
    pub span: bool,
    pub snippet: bool,
    pub visibility: bool,
    pub signature: bool,
    pub role: bool,
}

impl Default for FieldSet {
    fn default() -> Self {
        Self {
            file: true,
            id: false,
            locator: false,
            module: false,
            repo: false,
            span: false,
            snippet: false,
            visibility: false,
            signature: false,
            role: false,
        }
    }
}

impl FieldSet {
    pub fn with_id(mut self) -> Self {
        self.id = true;
        self
    }

    pub fn with_locator(mut self) -> Self {
        self.locator = true;
        self
    }

    pub fn all() -> Self {
        Self {
            file: true,
            id: true,
            locator: true,
            module: true,
            repo: true,
            span: true,
            snippet: true,
            visibility: true,
            signature: true,
            role: true,
        }
    }

    pub fn none() -> Self {
        Self {
            file: false,
            id: false,
            locator: false,
            module: false,
            repo: false,
            span: false,
            snippet: false,
            visibility: false,
            signature: false,
            role: false,
        }
    }

    pub fn parse(input: &str) -> Self {
        match input.trim() {
            "all" | "full" => Self::all(),
            "none" => Self::none(),
            s => {
                let mut fs = Self::none();
                for field in s.split(',') {
                    match field.trim() {
                        "file" => fs.file = true,
                        "id" => fs.id = true,
                        "locator" => fs.locator = true,
                        "module" => fs.module = true,
                        "repo" => fs.repo = true,
                        "span" => fs.span = true,
                        "snippet" => fs.snippet = true,
                        "visibility" => fs.visibility = true,
                        "signature" => fs.signature = true,
                        "role" => fs.role = true,
                        _ => {}
                    }
                }
                fs
            }
        }
    }

    pub fn from_config(fields: &[String]) -> Self {
        Self::parse(&fields.join(","))
    }
}
