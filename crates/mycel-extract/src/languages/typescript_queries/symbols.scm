
(export_statement (function_declaration name: (identifier) @fn.name) @fn.def) @exported
(export_statement (class_declaration name: (type_identifier) @class.name) @class.def) @exported
(export_statement (interface_declaration name: (type_identifier) @interface.name) @interface.def) @exported
(export_statement (type_alias_declaration name: (type_identifier) @type.name) @type.def) @exported
(function_declaration name: (identifier) @fn.name) @fn.def
(method_definition name: (property_identifier) @method.name) @method.def
(class_declaration name: (type_identifier) @class.name) @class.def
(interface_declaration name: (type_identifier) @interface.name) @interface.def
(type_alias_declaration name: (type_identifier) @type.name) @type.def
