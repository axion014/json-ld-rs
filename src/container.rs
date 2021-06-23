use crate::error::{JsonLdError, JsonLdErrorCode::*};

#[derive(Clone, Hash, Eq, PartialEq, Debug)]
pub enum Container {
	List,
	Unordered(UnorderedContainer)
}

impl Container {
	pub fn is_graph(&self) -> bool {
		if let Container::Unordered(UnorderedContainer {
			kind: ContainerKind::GraphContainer(GraphContainer { is_graph, .. }), ..
		}) = self {
			*is_graph
		} else {
			false
		}
	}

	pub fn is_id(&self) -> bool {
		if let IdContainer!() = self { true } else { false }
	}

	pub fn is_index(&self) -> bool {
		if let IndexContainer!() = self { true } else { false }
	}

	pub fn is_set(&self) -> bool {
		if let Container::Unordered(UnorderedContainer { is_set: true, .. }) = self { true } else { false }
	}

	pub fn get_kind_str(&self) -> Option<&str> {
		if let Container::Unordered(UnorderedContainer { kind, .. }) = self { kind.get_kind_str() } else { None }
	}
}

#[derive(Clone, Hash, Eq, PartialEq, Debug)]
pub struct UnorderedContainer {
	pub is_set: bool,
	pub kind: ContainerKind
}

#[derive(Clone, Hash, Eq, PartialEq, Debug)]
pub enum ContainerKind {
	GraphContainer(GraphContainer),
	Language,
	Type
}

impl ContainerKind {
	pub fn get_kind_str(&self) -> Option<&str> {
		match self {
			Self::GraphContainer(GraphContainer { kind: Some(kind), .. }) => Some(kind.as_str()),
			Self::Language => Some("@language"),
			Self::Type => Some("@type"),
			_ => None
		}
	}
}

#[derive(Clone, Hash, Eq, PartialEq, Debug)]
pub struct GraphContainer {
	pub is_graph: bool,
	pub kind: Option<GraphContainerKind>
}

#[derive(Clone, Hash, Eq, PartialEq, Debug)]
pub enum GraphContainerKind {
	Id,
	Index
}

impl GraphContainerKind {
	pub fn as_str(&self) -> &str {
		match self {
			Self::Id => "@id",
			Self::Index => "@index"
		}
	}
}

pub fn parse_container<'a>(containers: impl IntoIterator<Item = &'a str>) -> Result<Container, JsonLdError> {
	let mut is_set = false;
	let mut is_graph = false;
	let mut container_type = None;
	for container in containers {
		match container {
			"@set" if is_set => return Err(err!(InvalidContainerMapping, "found multiple @set values")),
			"@set" => is_set = true,
			"@graph" if is_graph => return Err(err!(InvalidContainerMapping, "found multiple @graph values")),
			"@graph" => is_graph = true,
			_ if container_type.is_some() => return Err(err!(InvalidContainerMapping, "found multiple non-@set/@graph values")),
			container => container_type = Some(container)
		}
	}
	match container_type {
		Some("@list") if is_set || is_graph => {
			Err(err!(InvalidContainerMapping, "@list container can't be composed with other container types"))
		},
		Some("@list") => Ok(Container::List),
		None if !(is_set || is_graph) => Err(err!(InvalidContainerMapping, "@container cannot be an empty array")),
		_ => Ok(Container::Unordered(UnorderedContainer {
			is_set, kind: match container_type {
				Some("@language" | "@type") if is_graph => {
					return Err(err!(InvalidContainerMapping,
						"@graph container can't be composed with container types other than @id, @index, and @set"));
				},
				Some("@language") => ContainerKind::Language,
				Some("@type") => ContainerKind::Type,
				_ => ContainerKind::GraphContainer(GraphContainer {
					is_graph, kind: match container_type {
						Some("@id") => Some(GraphContainerKind::Id),
						Some("@index") => Some(GraphContainerKind::Index),
						None => None,
						_ => return Err(err!(InvalidContainerMapping, "found unknown container type"))
					}
				})
			}
		}))
	}
}