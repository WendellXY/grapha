pub mod pass;
pub mod rust;
pub mod swift;
pub mod toml_rules;

use std::path::PathBuf;

use crate::graph::{FlowDirection, TerminalKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Classification {
    pub terminal_kind: TerminalKind,
    pub direction: FlowDirection,
    pub operation: String,
}

#[derive(Debug, Clone)]
pub struct ClassifyContext {
    #[allow(dead_code)]
    pub source_node: String,
    #[allow(dead_code)]
    pub file: PathBuf,
    #[allow(dead_code)]
    pub arguments: Vec<String>,
}

pub trait Classifier: Send + Sync {
    fn classify(&self, call_target: &str, context: &ClassifyContext) -> Option<Classification>;
}

pub struct CompositeClassifier {
    classifiers: Vec<Box<dyn Classifier>>,
}

impl CompositeClassifier {
    pub fn new(classifiers: Vec<Box<dyn Classifier>>) -> Self {
        Self { classifiers }
    }

    pub fn classify(&self, call_target: &str, context: &ClassifyContext) -> Option<Classification> {
        self.classifiers
            .iter()
            .find_map(|c| c.classify(call_target, context))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct AlwaysMatch {
        classification: Classification,
    }

    impl Classifier for AlwaysMatch {
        fn classify(
            &self,
            _call_target: &str,
            _context: &ClassifyContext,
        ) -> Option<Classification> {
            Some(self.classification.clone())
        }
    }

    struct NeverMatch;

    impl Classifier for NeverMatch {
        fn classify(
            &self,
            _call_target: &str,
            _context: &ClassifyContext,
        ) -> Option<Classification> {
            None
        }
    }

    fn test_context() -> ClassifyContext {
        ClassifyContext {
            source_node: "test::caller".to_string(),
            file: PathBuf::from("test.rs"),
            arguments: vec![],
        }
    }

    #[test]
    fn composite_returns_first_match() {
        let c = CompositeClassifier::new(vec![Box::new(AlwaysMatch {
            classification: Classification {
                terminal_kind: TerminalKind::Network,
                direction: FlowDirection::Read,
                operation: "HTTP_GET".to_string(),
            },
        })]);
        let result = c.classify("something", &test_context());
        assert!(result.is_some());
        assert_eq!(result.unwrap().terminal_kind, TerminalKind::Network);
    }

    #[test]
    fn composite_returns_none_when_no_match() {
        let c = CompositeClassifier::new(vec![Box::new(NeverMatch)]);
        let result = c.classify("something", &test_context());
        assert!(result.is_none());
    }

    #[test]
    fn first_classifier_wins() {
        let c = CompositeClassifier::new(vec![
            Box::new(AlwaysMatch {
                classification: Classification {
                    terminal_kind: TerminalKind::Network,
                    direction: FlowDirection::Read,
                    operation: "FIRST".to_string(),
                },
            }),
            Box::new(AlwaysMatch {
                classification: Classification {
                    terminal_kind: TerminalKind::Persistence,
                    direction: FlowDirection::Write,
                    operation: "SECOND".to_string(),
                },
            }),
        ]);
        let result = c.classify("something", &test_context()).unwrap();
        assert_eq!(result.operation, "FIRST");
        assert_eq!(result.terminal_kind, TerminalKind::Network);
    }
}
