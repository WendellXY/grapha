use grapha_core::classify::{Classification, Classifier, ClassifyContext};
use grapha_core::graph::{FlowDirection, TerminalKind};
use grapha_core::semantic::TerminalEffect;

#[cfg_attr(not(test), allow(dead_code))]
pub struct SwiftClassifier;

impl SwiftClassifier {
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn new() -> Self {
        Self
    }
}

impl Classifier for SwiftClassifier {
    fn classify(&self, call_target: &str, _context: &ClassifyContext) -> Option<Classification> {
        classify_swift(call_target)
    }
}

pub(crate) fn terminal_effect_for_target(target: &str) -> Option<TerminalEffect> {
    classify_swift(target).map(|classification| TerminalEffect {
        terminal_kind: classification.terminal_kind,
        direction: classification.direction,
        operation: classification.operation,
    })
}

fn classify_swift(target: &str) -> Option<Classification> {
    if matches_any(target, &["URLSession", "AF.", "Alamofire", "Moya"]) {
        let direction = if contains_any(target, &["download", "data(", "get"]) {
            FlowDirection::Read
        } else if contains_any(target, &["upload", "post", "put", "delete"]) {
            FlowDirection::Write
        } else {
            FlowDirection::ReadWrite
        };
        return Some(Classification {
            terminal_kind: TerminalKind::Network,
            direction,
            operation: "HTTP".to_string(),
        });
    }

    if matches_any(
        target,
        &[
            "NSManagedObjectContext",
            "NSFetchRequest",
            "NSPersistentContainer",
        ],
    ) {
        let direction = if contains_any(target, &["fetch", "count"]) {
            FlowDirection::Read
        } else if contains_any(target, &["save", "delete", "insert"]) {
            FlowDirection::Write
        } else {
            FlowDirection::ReadWrite
        };
        return Some(Classification {
            terminal_kind: TerminalKind::Persistence,
            direction,
            operation: "CoreData".to_string(),
        });
    }

    if matches_any(target, &["realm", "Realm"]) {
        let direction = if contains_any(target, &["objects", "object("]) {
            FlowDirection::Read
        } else if contains_any(target, &["write", "add", "delete"]) {
            FlowDirection::Write
        } else {
            FlowDirection::ReadWrite
        };
        return Some(Classification {
            terminal_kind: TerminalKind::Persistence,
            direction,
            operation: "Realm".to_string(),
        });
    }

    if matches_any(target, &["UserDefaults"]) {
        let direction = if contains_any(
            target,
            &["string(", "bool(", "integer(", "object(", "value("],
        ) {
            FlowDirection::Read
        } else if contains_any(target, &["set(", "setValue", "removeObject"]) {
            FlowDirection::Write
        } else {
            FlowDirection::ReadWrite
        };
        return Some(Classification {
            terminal_kind: TerminalKind::Persistence,
            direction,
            operation: "UserDefaults".to_string(),
        });
    }

    if matches_any(target, &["KeychainWrapper", "SecItem", "Keychain"]) {
        let direction = if contains_any(target, &["get", "string(", "data(", "Copy"]) {
            FlowDirection::Read
        } else if contains_any(target, &["set", "add", "update", "delete", "remove"]) {
            FlowDirection::Write
        } else {
            FlowDirection::ReadWrite
        };
        return Some(Classification {
            terminal_kind: TerminalKind::Keychain,
            direction,
            operation: "Keychain".to_string(),
        });
    }

    if matches_any(target, &["NotificationCenter"]) {
        let direction = if contains_any(target, &["post"]) {
            FlowDirection::Write
        } else if contains_any(target, &["addObserver", "observe"]) {
            FlowDirection::Read
        } else {
            FlowDirection::ReadWrite
        };
        return Some(Classification {
            terminal_kind: TerminalKind::Event,
            direction,
            operation: if contains_any(target, &["post"]) {
                "publish".to_string()
            } else {
                "subscribe".to_string()
            },
        });
    }

    if matches_any(target, &["PassthroughSubject", "CurrentValueSubject"]) {
        return Some(Classification {
            terminal_kind: TerminalKind::Event,
            direction: FlowDirection::Write,
            operation: "publish".to_string(),
        });
    }

    if matches_any(target, &["NSCache"]) {
        let direction = if contains_any(target, &["object(", "value("]) {
            FlowDirection::Read
        } else if contains_any(target, &["setObject", "removeObject", "removeAll"]) {
            FlowDirection::Write
        } else {
            FlowDirection::ReadWrite
        };
        return Some(Classification {
            terminal_kind: TerminalKind::Cache,
            direction,
            operation: "Cache".to_string(),
        });
    }

    None
}

fn matches_any(target: &str, patterns: &[&str]) -> bool {
    patterns.iter().any(|pattern| target.contains(pattern))
}

fn contains_any(target: &str, patterns: &[&str]) -> bool {
    patterns.iter().any(|pattern| target.contains(pattern))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn ctx() -> ClassifyContext {
        ClassifyContext {
            source_node: "test::caller".to_string(),
            file: PathBuf::from("Test.swift"),
            arguments: vec![],
        }
    }

    #[test]
    fn classifies_urlsession_as_network() {
        let classifier = SwiftClassifier::new();
        let result = classifier
            .classify("URLSession.shared.dataTask", &ctx())
            .unwrap();
        assert_eq!(result.terminal_kind, TerminalKind::Network);
        assert_eq!(result.operation, "HTTP");
    }
}
