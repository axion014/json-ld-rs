macro_rules! err {
	($code:expr) => {
		crate::error::JsonLdError {
			code: $code,
			description: None,
			cause: None
		}
	};
	($code:expr, $desc:expr) => {
		crate::error::JsonLdError {
			code: $code,
			description: Some($desc.into()),
			cause: None
		}
	};
	($code:expr, , $cause:expr) => {
		crate::error::JsonLdError {
			code: $code,
			description: None,
			cause: Some(Box::new($cause))
		}
	};
	($code:expr, $desc:expr, $cause:expr) => {
		crate::error::JsonLdError {
			code: $code,
			description: Some($desc.into()),
			cause: Some(Box::new($cause))
		}
	};
}

macro_rules! expand_iri {
	($active_context:expr, $value:expr) => {
		expand_iri!($active_context, $value, false)
	};
	($active_context:expr, $value:expr, $document_relative:expr) => {
		expand_iri!($active_context, $value, $document_relative, true)
	};
	($active_context:expr, $value:expr, $document_relative:expr, $vocab:expr) => {
		// FIXME: Waiting for Never type to arrive
		expand_iri::<_, for<'doc> fn(&'doc str, &'doc Option<LoadDocumentOptions>) -> BoxFuture<'doc, Result<RemoteDocument<T>>>>(
			crate::expand::IRIExpansionArguments::Normal($active_context),
			$value,
			$document_relative,
			$vocab
		)
	};
}

macro_rules! process_context_iri {
	($active_context:expr, $value:expr, $local_context:expr, $defined:expr, $options:expr) => {
		process_context_iri!($active_context, $value, false, true, $local_context, $defined, $options)
	};
	($active_context:expr, $value:expr, $document_relative:expr, $vocab:expr, $local_context:expr, $defined:expr, $options:expr) => {
		expand_iri(
			crate::expand::IRIExpansionArguments::DefineTerms {
				active_context: $active_context,
				local_context: $local_context,
				defined: $defined,
				options: $options
			},
			$value,
			$document_relative,
			$vocab
		)
	};
}

macro_rules! container {
	($kind:ident, $is_set:expr) => {
		crate::container::Container::Unordered(crate::container::UnorderedContainer {
			kind: crate::container::ContainerKind::$kind,
			is_set: $is_set
		})
	};
	(None, $is_set:expr, $is_graph:expr) => {
		container!(std::option::Option::None, $is_set, $is_graph)
	};
	($kind:ident, $is_set:expr, $is_graph:expr) => {
		container!(Some(crate::container::GraphContainerKind::$kind), $is_set, $is_graph)
	};
	($kind:expr, $is_set:expr, $is_graph:expr) => {
		crate::container::Container::Unordered(crate::container::UnorderedContainer {
			kind: crate::container::ContainerKind::GraphContainer(crate::container::GraphContainer { kind: $kind, is_graph: $is_graph }),
			is_set: $is_set
		})
	};
	(None) => {
		container!(None, false, false)
	};
	(list) => {
		crate::container::Container::List
	};
	(set) => {
		container!(None, true, false)
	};
	(graph) => {
		container!(None, false, true)
	};
	(set_graph) => {
		container!(None, true, true)
	};
	(id) => {
		container!(Id, false, false)
	};
	(index) => {
		container!(Index, false, false)
	};
	(ids) => {
		container!(Id, true, false)
	};
	(indexes) => {
		container!(Index, true, false)
	};
	(id_graph) => {
		container!(Id, false, true)
	};
	(index_graph) => {
		container!(Index, false, true)
	};
	(ids_graph) => {
		container!(Id, true, true)
	};
	(indexes_graph) => {
		container!(Index, true, true)
	};
	(language) => {
		container!(Language, false)
	};
	(type) => {
		container!(Type, false)
	};
	(languages) => {
		container!(Language, true)
	};
	(types) => {
		container!(Type, true)
	};
}

macro_rules! IdContainer {
	() => {
		crate::container::Container::Unordered(crate::container::UnorderedContainer {
			kind: crate::container::ContainerKind::GraphContainer(crate::container::GraphContainer {
				kind: Some(crate::container::GraphContainerKind::Id),
				..
			}),
			..
		})
	};
}

macro_rules! IndexContainer {
	() => {
		crate::container::Container::Unordered(crate::container::UnorderedContainer {
			kind: crate::container::ContainerKind::GraphContainer(crate::container::GraphContainer {
				kind: Some(crate::container::GraphContainerKind::Index),
				..
			}),
			..
		})
	};
}

macro_rules! LanguageContainer {
	() => {
		crate::container::Container::Unordered(crate::container::UnorderedContainer {
			kind: crate::container::ContainerKind::Language,
			..
		})
	};
}

macro_rules! TypeContainer {
	() => {
		crate::container::Container::Unordered(crate::container::UnorderedContainer {
			kind: crate::container::ContainerKind::Type,
			..
		})
	};
}
