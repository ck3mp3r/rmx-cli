use anyhow::{anyhow, Error};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, u8};

use super::parser::{SplitType, Token};

#[derive(Serialize, Deserialize, Debug, Default, Clone)]
pub enum FlexDirection {
    #[serde(rename = "row")]
    #[default]
    Row,
    #[serde(rename = "column")]
    Column,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub(crate) struct Pane {
    #[serde(default)]
    pub(crate) flex_direction: FlexDirection,
    #[serde(default = "default_flex")]
    pub(crate) flex: usize,
    #[serde(default = "default_path")]
    pub(crate) path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) style: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) commands: Vec<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub(crate) env: HashMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) panes: Option<Vec<Pane>>,
}

fn default_flex() -> usize {
    1
}

fn default_path() -> Option<String> {
    Some(".".to_string())
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct Window {
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) flex_direction: FlexDirection,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) panes: Vec<Pane>,
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct Session {
    pub(crate) name: String,
    #[serde(default = "default_path")]
    pub(crate) path: Option<String>,
    #[serde(default, alias = "commands", skip_serializing_if = "Vec::is_empty")]
    pub(crate) startup: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) shutdown: Vec<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub(crate) env: HashMap<String, String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) windows: Vec<Window>,
}

impl FlexDirection {
    pub(crate) fn from_split_type(split_type: &SplitType) -> Self {
        match split_type {
            SplitType::Horizontal => Self::Column,
            SplitType::Vertical => Self::Row,
        }
    }
}

impl Pane {
    fn from_tokens(
        children: &[Token], // Use slice instead of Vec reference
        flex_direction: FlexDirection,
    ) -> Option<Vec<Pane>> {
        if children.is_empty() {
            return None;
        }

        let dimension_selector = match flex_direction {
            FlexDirection::Row => |c: &Token| c.dimensions.height as usize,
            FlexDirection::Column => |c: &Token| c.dimensions.width as usize,
        };

        let dimensions: Vec<usize> = children
            .iter()
            .map(|c| dimension_selector(c))
            .map(round)
            .collect();

        let gcd = gcd_vec(&dimensions);
        log::trace!("gcd of dimensions: {:?}", gcd);

        // Calculate initial flex values
        let flex_values: Vec<usize> = children
            .iter()
            .map(|token| dimension_selector(token) / gcd)
            .collect();

        // Normalize flex values using the GCD
        let flex_gcd = gcd_vec(&flex_values);
        log::trace!("gcd of flex_values: {:?}", flex_gcd);

        // Creating panes with normalized flex values
        let panes: Vec<Pane> = children
            .iter()
            .zip(flex_values.iter())
            .map(|(token, &flex_value)| {
                let normalized_flex_value = (flex_value / flex_gcd).max(1);

                let pane_flex_direction = token
                    .split_type
                    .as_ref()
                    .map(FlexDirection::from_split_type)
                    .unwrap_or(FlexDirection::default());

                Pane {
                    flex_direction: pane_flex_direction.clone(),
                    flex: normalized_flex_value,
                    style: None,
                    path: Some(".".to_string()),
                    commands: vec![],
                    env: HashMap::new(),
                    panes: Pane::from_tokens(&token.children, pane_flex_direction),
                }
            })
            .inspect(|pane| log::trace!("pane: {:?}", pane))
            .collect();

        Some(panes)
    }
}

impl Window {
    fn from_tokens(token: &Token) -> Self {
        let pane_flex_direction = token
            .split_type
            .as_ref()
            .map(FlexDirection::from_split_type);
        Self {
            name: token.name.clone().unwrap_or_else(|| "foo".to_string()),
            flex_direction: pane_flex_direction
                .clone()
                .unwrap_or(FlexDirection::default()),
            panes: Pane::from_tokens(
                &token.children,
                pane_flex_direction.unwrap_or(FlexDirection::default()),
            )
            .unwrap_or_else(Vec::new),
        }
    }

    pub fn validate(&self) -> Result<(), Error> {
        if self.panes.is_empty() {
            return Err(anyhow!("Panes cannot be empty"));
        }

        Ok(())
    }
}

impl Session {
    pub(crate) fn from_tokens(name: &String, tokens: &Vec<Token>) -> Self {
        Self {
            name: name.clone(),
            startup: vec![],
            shutdown: vec![],
            env: HashMap::new(),
            path: Some(".".to_string()),
            windows: tokens
                .iter()
                .map(|token| {
                    log::trace!("{:?}", token);
                    Window::from_tokens(token)
                })
                .collect(),
        }
    }

    pub fn validate(&self) -> Result<(), Vec<Error>> {
        let mut errors: Vec<Error> = Vec::new();
        if self.windows.is_empty() {
            errors.push(anyhow!("Windows cannot be empty"));
        }

        let window_errors: Vec<Error> = self
            .windows
            .iter()
            .filter_map(|w| w.validate().err())
            .collect();

        errors.extend(window_errors);
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

fn gcd(a: usize, b: usize) -> usize {
    if b == 0 {
        a
    } else {
        gcd(b, a % b)
    }
}

fn gcd_vec(numbers: &Vec<usize>) -> usize {
    if numbers.is_empty() || numbers.iter().all(|&x| x == 0) {
        return 1; // Return 1 if vector is empty or all zeros
    }
    numbers.iter().fold(0, |acc, &x| gcd(acc, x))
}

// Function to round a number to the nearest multiple of base
fn round(number: usize) -> usize {
    let base = 3;
    let remainder = number % base;
    if remainder >= base / 2 {
        number + base - remainder
    } else {
        number - remainder
    }
}
