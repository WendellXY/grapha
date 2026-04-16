use std::path::Path;

use grapha_core::ModuleMap;

pub fn discover_swift_modules(root: &Path) -> ModuleMap {
    let mut modules = ModuleMap::new();
    discover_swift_packages_recursive(root, &mut modules);
    modules
}

fn discover_swift_packages_recursive(dir: &Path, modules: &mut ModuleMap) {
    let package_swift = dir.join("Package.swift");
    if package_swift.is_file() {
        let module_name = dir
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("unknown")
            .to_string();
        let sources_dir = dir.join("Sources");
        let source_dir = if sources_dir.is_dir() {
            sources_dir
        } else {
            dir.to_path_buf()
        };
        modules
            .modules
            .entry(module_name)
            .or_default()
            .push(source_dir);

        // Register test targets: each subdirectory under Tests/ becomes a module.
        // If Tests/ has no subdirectories, register it as "<PackageName>Tests".
        let tests_dir = dir.join("Tests");
        if tests_dir.is_dir() {
            let mut found_subdirs = false;
            if let Ok(entries) = std::fs::read_dir(&tests_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        let name = entry.file_name();
                        let name = name.to_string_lossy();
                        if !name.starts_with('.') {
                            modules
                                .modules
                                .entry(name.to_string())
                                .or_default()
                                .push(path);
                            found_subdirs = true;
                        }
                    }
                }
            }
            if !found_subdirs {
                let pkg_name = dir
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unknown");
                modules
                    .modules
                    .entry(format!("{pkg_name}Tests"))
                    .or_default()
                    .push(tests_dir);
            }
        }

        return;
    }

    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.')
            || name == "node_modules"
            || name == "build"
            || name == "DerivedData"
            || name == "Pods"
        {
            continue;
        }
        discover_swift_packages_recursive(&path, modules);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn discovers_swift_package() {
        let dir = tempfile::tempdir().unwrap();
        let pkg_dir = dir.path().join("MyPackage");
        let sources_dir = pkg_dir.join("Sources");
        fs::create_dir_all(&sources_dir).unwrap();
        fs::write(pkg_dir.join("Package.swift"), "// swift-tools-version:5.5").unwrap();

        let modules = discover_swift_modules(dir.path());
        assert!(modules.modules.contains_key("MyPackage"));
    }

    #[test]
    fn discovers_test_targets_as_modules() {
        let dir = tempfile::tempdir().unwrap();
        let pkg_dir = dir.path().join("FrameBase");
        let sources_dir = pkg_dir.join("Sources");
        let tests_dir = pkg_dir.join("Tests").join("FrameBaseTests");
        fs::create_dir_all(&sources_dir).unwrap();
        fs::create_dir_all(&tests_dir).unwrap();
        fs::write(pkg_dir.join("Package.swift"), "// swift-tools-version:5.5").unwrap();

        let modules = discover_swift_modules(dir.path());
        assert!(modules.modules.contains_key("FrameBase"));
        assert!(
            modules.modules.contains_key("FrameBaseTests"),
            "test subdirectory should be registered as a module"
        );
    }

    #[test]
    fn discovers_test_fallback_when_no_subdirs() {
        let dir = tempfile::tempdir().unwrap();
        let pkg_dir = dir.path().join("MyPkg");
        let sources_dir = pkg_dir.join("Sources");
        let tests_dir = pkg_dir.join("Tests");
        fs::create_dir_all(&sources_dir).unwrap();
        fs::create_dir_all(&tests_dir).unwrap();
        // Put a file directly in Tests/ with no subdirs
        fs::write(tests_dir.join("MyPkgTests.swift"), "import XCTest").unwrap();
        fs::write(pkg_dir.join("Package.swift"), "// swift-tools-version:5.5").unwrap();

        let modules = discover_swift_modules(dir.path());
        assert!(modules.modules.contains_key("MyPkg"));
        assert!(
            modules.modules.contains_key("MyPkgTests"),
            "Tests/ with no subdirs should fallback to <Package>Tests"
        );
    }
}
