import Lean

namespace NiaValueInference

/-
The semantic model begins after name resolution. It models the type-shape
constraints that carry an expected value type to a constructor child. The
compiler matrix checks the corresponding parser, checker, const evaluator,
and code generator paths; it is not a proof of the implementation.
-/

inductive Primitive where
  | i8 | i16 | i32 | i64 | i128 | isize
  | u8 | u16 | u32 | u64 | u128 | usize
  | f32 | f64 | char | bool | never
  deriving DecidableEq, Repr

def Primitive.isInteger (p : Primitive) : Bool :=
  match p with
  | .i8 | .i16 | .i32 | .i64 | .i128 | .isize
  | .u8 | .u16 | .u32 | .u64 | .u128 | .usize => true
  | _ => false

def Primitive.isFloat (p : Primitive) : Bool :=
  match p with
  | .f32 | .f64 => true
  | _ => false

structure DefinitionId where
  moduleName : String
  itemName : String
  deriving DecidableEq, Repr

inductive ValueType where
  | unit
  | scalar (primitive : Primitive)
  | tuple (first second : ValueType)
  | array (length : Nat) (element : ValueType)
  | optional (element : ValueType)
  | errorUnion (error value : ValueType)
  | pointer (isReadonly : Bool) (element : ValueType)
  | slice (isReadonly : Bool) (element : ValueType)
  | nominal (definition : DefinitionId) (argument : ValueType)
  | enumeration (definition : DefinitionId)
  deriving DecidableEq, Repr
inductive Literal where
  | integerDefault
  | integerSuffix (primitive : Primitive)
  | integerNegative
  | floatDefault
  | floatSuffix (primitive : Primitive)
  | character
  | byteCharacter
  | string (length : Nat)
  | byteString (length : Nat)
  | boolean
  | null
  | emptyArray
  | unitValue
  deriving DecidableEq, Repr

def literalType (literal : Literal) (expected : Option ValueType) : Option ValueType :=
  match literal with
  | .integerDefault | .integerNegative =>
      match expected with
      | none => some (.scalar .i32)
      | some (.scalar primitive) => if primitive.isInteger then some (.scalar primitive) else none
      | some _ => none
  | .integerSuffix primitive =>
      match expected with
      | none => some (.scalar primitive)
      | some (.scalar expectedPrimitive) =>
          if primitive = expectedPrimitive && primitive.isInteger then some (.scalar primitive)
          else none
      | some _ => none
  | .floatDefault =>
      match expected with
      | none => some (.scalar .f64)
      | some (.scalar primitive) => if primitive.isFloat then some (.scalar primitive) else none
      | some _ => none
  | .floatSuffix primitive =>
      match expected with
      | none => some (.scalar primitive)
      | some (.scalar expectedPrimitive) =>
          if primitive = expectedPrimitive && primitive.isFloat then some (.scalar primitive)
          else none
      | some _ => none
  | .character =>
      match expected with
      | none | some (.scalar .char) => some (.scalar .char)
      | some _ => none
  | .byteCharacter =>
      match expected with
      | none | some (.scalar .u8) => some (.scalar .u8)
      | some _ => none
  | .string length =>
      match expected with
      | none => some (.array length (.scalar .char))
      | some (.array expectedLength (.scalar .char)) =>
          if length = expectedLength then some (.array length (.scalar .char)) else none
      | some _ => none
  | .byteString length =>
      match expected with
      | none => some (.array length (.scalar .u8))
      | some (.array expectedLength (.scalar .u8)) =>
          if length = expectedLength then some (.array length (.scalar .u8)) else none
      | some _ => none
  | .boolean =>
      match expected with
      | none | some (.scalar .bool) => some (.scalar .bool)
      | some _ => none
  | .null =>
      match expected with
      | some (.optional element) => some (.optional element)
      | none | some _ => none
  | .emptyArray =>
      match expected with
      | some (.array 0 element) => some (.array 0 element)
      | none | some _ => none
  | .unitValue =>
      match expected with
      | none | some .unit => some .unit
      | some _ => none

inductive AggregateSpelling where
  | typed | qualified | omitted | alias
  deriving DecidableEq, Repr

inductive AggregateKind where
  | structure | union | tupleStructure
  deriving DecidableEq, Repr

inductive Frame where
  | identityCall
  | tupleFirst (other : ValueType)
  | tupleSecond (other : ValueType)
  | arrayList (length : Nat)
  | arrayRepeat (length : Nat)
  | optionalSome
  | errorOk (error : ValueType)
  | errorError (value : ValueType)
  | blockTail
  | branch (other : ValueType)
  | pointer (isReadonly : Bool)
  | arrayToSlice (isReadonly : Bool) (length : Nat)
  | aggregateField (kind : AggregateKind) (spelling : AggregateSpelling) (definition : DefinitionId)
  | enumTuplePayload (definition : DefinitionId) (variant : String) (payload : ValueType)
  | enumNamedPayload (definition : DefinitionId) (variant field : String) (payload : ValueType)
  deriving DecidableEq, Repr

def buildFrame : Frame -> ValueType -> ValueType
  | .identityCall, value => value
  | .tupleFirst other, value => .tuple value other
  | .tupleSecond other, value => .tuple other value
  | .arrayList length, value => .array length value
  | .arrayRepeat length, value => .array length value
  | .optionalSome, value => .optional value
  | .errorOk error, value => .errorUnion error value
  | .errorError value, error => .errorUnion error value
  | .blockTail, value => value
  | .branch _, value => value
  | .pointer isReadonly, value => .pointer isReadonly value
  | .arrayToSlice isReadonly _, .array _ element => .slice isReadonly element
  | .arrayToSlice isReadonly _, value => .slice isReadonly value
  | .aggregateField _ _ definition, value => .nominal definition value
  | .enumTuplePayload definition _ _, _ => .enumeration definition
  | .enumNamedPayload definition _ _ _, _ => .enumeration definition

def projectFrame (frame : Frame) (expected : ValueType) : Option ValueType :=
  match frame with
  | .identityCall => some expected
  | .tupleFirst other =>
      match expected with
      | .tuple first second => if other = second then some first else none
      | _ => none
  | .tupleSecond other =>
      match expected with
      | .tuple first second => if other = first then some second else none
      | _ => none
  | .arrayList length | .arrayRepeat length =>
      match expected with
      | .array expectedLength element => if length = expectedLength then some element else none
      | _ => none
  | .optionalSome =>
      match expected with
      | .optional element => some element
      | _ => none
  | .errorOk error =>
      match expected with
      | .errorUnion expectedError value => if error = expectedError then some value else none
      | _ => none
  | .errorError value =>
      match expected with
      | .errorUnion error expectedValue => if value = expectedValue then some error else none
      | _ => none
  | .blockTail => some expected
  | .branch other => if other = expected then some expected else none
  | .pointer isReadonly =>
      match expected with
      | .pointer expectedReadonly element => if isReadonly = expectedReadonly then some element else none
      | _ => none
  | .arrayToSlice isReadonly length =>
      match expected with
      | .slice expectedReadonly element => if isReadonly = expectedReadonly then some (.array length element) else none
      | _ => none
  | .aggregateField _ _ definition =>
      match expected with
      | .nominal expectedDefinition element => if definition = expectedDefinition then some element else none
      | _ => none
  | .enumTuplePayload definition _ payload =>
      match expected with
      | .enumeration expectedDefinition => if definition = expectedDefinition then some payload else none
      | _ => none
  | .enumNamedPayload definition _ _ payload =>
      match expected with
      | .enumeration expectedDefinition => if definition = expectedDefinition then some payload else none
      | _ => none

def projectFrames : List Frame -> ValueType -> Option ValueType
  | [], expected => some expected
  | frame :: rest, expected => (projectFrame frame expected).bind (projectFrames rest)

def buildFrames : List Frame -> ValueType -> ValueType
  | [], value => value
  | frame :: rest, value => buildFrame frame (buildFrames rest value)

theorem build_frame_after_projection (frame : Frame) (expected child : ValueType)
    (h : projectFrame frame expected = some child) : buildFrame frame child = expected := by
  cases frame <;> cases expected <;> simp_all [projectFrame, buildFrame]
  all_goals cases child <;> simp_all

theorem build_frames_after_projection (frames : List Frame) (expected child : ValueType)
    (h : projectFrames frames expected = some child) : buildFrames frames child = expected := by
  induction frames generalizing expected child with
  | nil => simp [projectFrames] at h; simp [buildFrames, h]
  | cons frame rest ih =>
      cases hframe : projectFrame frame expected with
      | none => simp [projectFrames, hframe] at h
      | some frameChild =>
          have hrest : projectFrames rest frameChild = some child := by simpa [projectFrames, hframe] using h
          have hframeBuild : buildFrame frame frameChild = expected := build_frame_after_projection frame expected frameChild hframe
          have hrestBuild : buildFrames rest child = frameChild := ih frameChild child hrest
          simp [buildFrames, hrestBuild, hframeBuild]
def inferFrames (frames : List Frame) (literal : Literal)
    (expected : Option ValueType) : Option ValueType :=
  match expected with
  | none => (literalType literal none).map (buildFrames frames)
  | some output => do
      let leafExpected <- projectFrames frames output
      let leaf <- literalType literal (some leafExpected)
      let inferred := buildFrames frames leaf
      if inferred = output then some output else none

theorem contextual_inference_preserves_expected
    (frames : List Frame) (literal : Literal) (output leaf : ValueType)
    (hprojection : projectFrames frames output = some leaf)
    (hliteral : literalType literal (some leaf) = some leaf) :
    inferFrames frames literal (some output) = some output := by
  simp [inferFrames, hprojection, hliteral,
    build_frames_after_projection frames output leaf hprojection]

theorem empty_array_requires_its_expected_length_and_element_type :
    literalType .emptyArray (some (.array 0 (.scalar .i32))) =
      some (.array 0 (.scalar .i32)) ∧ literalType .emptyArray none = none := by
  exact And.intro rfl rfl

theorem unit_tuple_has_the_unit_type : literalType .unitValue none = some .unit := rfl

theorem string_literal_auto_address_projects_through_slice_context :
    inferFrames [.arrayToSlice true 2] (.string 2)
      (some (.slice true (.scalar .char))) = some (.slice true (.scalar .char)) := rfl

theorem byte_string_literal_auto_address_projects_through_slice_context :
    inferFrames [.arrayToSlice true 2] (.byteString 2)
      (some (.slice true (.scalar .u8))) = some (.slice true (.scalar .u8)) := rfl

theorem default_integer_literal_is_i32 : literalType .integerDefault none = some (.scalar .i32) := rfl

theorem default_float_literal_is_f64 : literalType .floatDefault none = some (.scalar .f64) := rfl

theorem null_requires_an_optional_context : literalType .null none = none := rfl

theorem incompatible_literal_context_is_rejected :
    literalType .integerDefault (some (.scalar .bool)) = none := by decide

def localOwner : DefinitionId := { moduleName := "root", itemName := "Node" }
def importedOwner : DefinitionId := { moduleName := "api", itemName := "Node" }

theorem source_spelling_does_not_change_resolved_nominal_identity
    (kind : AggregateKind) (child : ValueType) :
    buildFrame (.aggregateField kind .typed localOwner) child =
      buildFrame (.aggregateField kind .qualified localOwner) child ∧
    buildFrame (.aggregateField kind .qualified localOwner) child =
      buildFrame (.aggregateField kind .omitted localOwner) child ∧
    buildFrame (.aggregateField kind .omitted localOwner) child =
      buildFrame (.aggregateField kind .alias localOwner) child := by
  exact And.intro rfl (And.intro rfl rfl)

theorem imported_declaration_identity_is_preserved (child : ValueType) :
    buildFrame (.aggregateField .structure .omitted importedOwner) child =
      .nominal importedOwner child := rfl

theorem distinct_module_declarations_remain_distinct (child : ValueType) :
    ValueType.nominal localOwner child ≠ ValueType.nominal importedOwner child := by
  intro h
  have hId : localOwner = importedOwner := by injection h
  simp [localOwner, importedOwner] at hId
/- The classification inventory mirrors an exhaustive ExprKind match in the
   Rust inference pass. Adding an AST variant therefore requires an explicit
   classification before the compiler builds, and the list must be reflected
   here and in the compiler matrix. -/
inductive AstExpressionTag where
  | error | integer | float | string | byteString | char | byteChar | raw | bool | null
  | ident | selfValue | pathRoot | underscore | typeTarget | traitTarget | bracketSuffix
  | tuple | closure | arrayLiteral | typedStructLiteral | qualifiedStructLiteral
  | omittedAggregateLiteral | omittedMember | unary | optionalSome | errorOk | errorErr
  | tryExpr | binary | assign | cast | call | qualified | field | tupleField | index
  | range | block | ifExpr | ifPattern | ifPatternChain | matchExpr
  deriving DecidableEq, Repr

def AstExpressionTag.name : AstExpressionTag -> String
  | .error => "error"
  | .integer => "integer"
  | .float => "float"
  | .string => "string"
  | .byteString => "byte_string"
  | .char => "char"
  | .byteChar => "byte_char"
  | .raw => "raw"
  | .bool => "bool"
  | .null => "null"
  | .ident => "ident"
  | .selfValue => "self_value"
  | .pathRoot => "path_root"
  | .underscore => "underscore"
  | .typeTarget => "type_target"
  | .traitTarget => "trait_target"
  | .bracketSuffix => "bracket_suffix"
  | .tuple => "tuple"
  | .closure => "closure"
  | .arrayLiteral => "array_literal"
  | .typedStructLiteral => "typed_struct_literal"
  | .qualifiedStructLiteral => "qualified_struct_literal"
  | .omittedAggregateLiteral => "omitted_aggregate_literal"
  | .omittedMember => "omitted_member"
  | .unary => "unary"
  | .optionalSome => "optional_some"
  | .errorOk => "error_ok"
  | .errorErr => "error_err"
  | .tryExpr => "try"
  | .binary => "binary"
  | .assign => "assign"
  | .cast => "cast"
  | .call => "call"
  | .qualified => "qualified"
  | .field => "field"
  | .tupleField => "tuple_field"
  | .index => "index"
  | .range => "range"
  | .block => "block"
  | .ifExpr => "if"
  | .ifPattern => "if_pattern"
  | .ifPatternChain => "if_pattern_chain"
  | .matchExpr => "match"

def allAstExpressionTags : List AstExpressionTag := [
  .error, .integer, .float, .string, .byteString, .char, .byteChar, .raw, .bool, .null,
  .ident, .selfValue, .pathRoot, .underscore, .typeTarget, .traitTarget, .bracketSuffix,
  .tuple, .closure, .arrayLiteral, .typedStructLiteral, .qualifiedStructLiteral,
  .omittedAggregateLiteral, .omittedMember, .unary, .optionalSome, .errorOk, .errorErr,
  .tryExpr, .binary, .assign, .cast, .call, .qualified, .field, .tupleField, .index,
  .range, .block, .ifExpr, .ifPattern, .ifPatternChain, .matchExpr
]

def astExpressionTags : List String := allAstExpressionTags.map AstExpressionTag.name

inductive ExpressionRole where
  | recovery | literal | constructor | operation | lookup
  deriving DecidableEq, Repr

def AstExpressionTag.role : AstExpressionTag -> ExpressionRole
  | .error => .recovery
  | .integer | .float | .string | .byteString | .char | .byteChar | .raw | .bool | .null => .literal
  | .tuple | .closure | .arrayLiteral | .typedStructLiteral | .qualifiedStructLiteral
  | .omittedAggregateLiteral | .omittedMember | .optionalSome | .errorOk | .errorErr => .constructor
  | .unary | .tryExpr | .binary | .assign | .cast | .call | .range | .block
  | .ifExpr | .ifPattern | .ifPatternChain | .matchExpr => .operation
  | .ident | .selfValue | .pathRoot | .underscore | .typeTarget | .traitTarget
  | .bracketSuffix | .qualified | .field | .tupleField | .index => .lookup

theorem every_ast_expression_form_has_a_classification :
    allAstExpressionTags.filter (fun tag =>
      tag.role == .recovery || tag.role == .literal || tag.role == .constructor ||
        tag.role == .operation || tag.role == .lookup) = allAstExpressionTags := by
  decide

inductive ConstructorTag where
  | identityCall | tupleFirst | tupleSecond | tupleThird | arrayList | arrayRepeat
  | optionalSome | errorOk | errorError | blockTail | ifThen | ifElse | matchFirst | matchSecond
  | pointerRef | arrayToSlice | emptyArray | unitTuple | emptyStruct
  | structTypedField | structQualifiedField | structOmittedField
  | unionTypedField | unionQualifiedField | unionOmittedField
  | tupleStructPayload | externalTupleStructPayload | aliasOmittedField
  | externalStructQualifiedField | externalStructOmittedField
  | enumUnitQualified | enumUnitOmitted | enumTupleQualified | enumTupleOmitted
  | enumNamedQualified | enumNamedOmitted
  deriving DecidableEq, Repr

def ConstructorTag.name : ConstructorTag -> String
  | .identityCall => "identity_call"
  | .tupleFirst => "tuple_first"
  | .tupleSecond => "tuple_second"
  | .tupleThird => "tuple_third"
  | .arrayList => "array_list"
  | .arrayRepeat => "array_repeat"
  | .optionalSome => "optional_some"
  | .errorOk => "error_ok"
  | .errorError => "error_error"
  | .blockTail => "block_tail"
  | .ifThen => "if_then"
  | .ifElse => "if_else"
  | .matchFirst => "match_first"
  | .matchSecond => "match_second"
  | .pointerRef => "pointer_ref"
  | .arrayToSlice => "array_to_slice"
  | .emptyArray => "array_empty"
  | .unitTuple => "unit_tuple"
  | .emptyStruct => "empty_struct"
  | .structTypedField => "struct_typed_field"
  | .structQualifiedField => "struct_qualified_field"
  | .structOmittedField => "struct_omitted_field"
  | .unionTypedField => "union_typed_field"
  | .unionQualifiedField => "union_qualified_field"
  | .unionOmittedField => "union_omitted_field"
  | .tupleStructPayload => "tuple_struct_payload"
  | .externalTupleStructPayload => "external_tuple_struct_payload"
  | .aliasOmittedField => "alias_omitted_field"
  | .externalStructQualifiedField => "external_struct_qualified_field"
  | .externalStructOmittedField => "external_struct_omitted_field"
  | .enumUnitQualified => "enum_unit_qualified"
  | .enumUnitOmitted => "enum_unit_omitted"
  | .enumTupleQualified => "enum_tuple_qualified"
  | .enumTupleOmitted => "enum_tuple_omitted"
  | .enumNamedQualified => "enum_named_qualified"
  | .enumNamedOmitted => "enum_named_omitted"

def ConstructorTag.frame : ConstructorTag -> Option Frame
  | .identityCall => some .identityCall
  | .tupleFirst => some (.tupleFirst (.scalar .i32))
  | .tupleSecond => some (.tupleSecond (.scalar .i32))
  | .tupleThird =>
      some (.tupleSecond (.scalar .i32))
  | .arrayList => some (.arrayList 1)
  | .arrayRepeat => some (.arrayRepeat 1)
  | .optionalSome => some .optionalSome
  | .errorOk => some (.errorOk (.scalar .i32))
  | .errorError => some (.errorError (.scalar .i32))
  | .blockTail => some .blockTail
  | .ifThen | .ifElse | .matchFirst | .matchSecond => some (.branch (.scalar .i32))
  | .pointerRef => some (.pointer true)
  | .arrayToSlice => some (.arrayToSlice true 2)
  | .structTypedField => some (.aggregateField .structure .typed localOwner)
  | .structQualifiedField => some (.aggregateField .structure .qualified localOwner)
  | .structOmittedField => some (.aggregateField .structure .omitted localOwner)
  | .unionTypedField => some (.aggregateField .union .typed localOwner)
  | .unionQualifiedField => some (.aggregateField .union .qualified importedOwner)
  | .unionOmittedField => some (.aggregateField .union .omitted localOwner)
  | .tupleStructPayload => some (.aggregateField .tupleStructure .typed localOwner)
  | .externalTupleStructPayload =>
      some (.aggregateField .tupleStructure .qualified importedOwner)
  | .aliasOmittedField => some (.aggregateField .structure .alias localOwner)
  | .externalStructQualifiedField =>
      some (.aggregateField .structure .qualified importedOwner)
  | .externalStructOmittedField =>
      some (.aggregateField .structure .omitted importedOwner)
  | .enumUnitQualified | .enumUnitOmitted => none
  | .emptyArray | .unitTuple | .emptyStruct => none
  | .enumTupleQualified | .enumTupleOmitted =>
      some (.enumTuplePayload importedOwner "Data" (.scalar .i64))
  | .enumNamedQualified | .enumNamedOmitted =>
      some (.enumNamedPayload importedOwner "Data" "value" (.scalar .i64))

def ConstructorTag.isGenericWrapper (tag : ConstructorTag) : Bool :=
  match tag with
  | .identityCall | .tupleFirst | .tupleSecond | .tupleThird | .arrayList | .arrayRepeat
  | .optionalSome | .errorOk | .errorError | .blockTail | .ifThen | .ifElse
  | .matchFirst | .matchSecond | .pointerRef | .arrayToSlice
  | .structTypedField | .structQualifiedField | .structOmittedField
  | .unionTypedField | .unionQualifiedField | .unionOmittedField | .tupleStructPayload
  | .externalTupleStructPayload | .aliasOmittedField | .externalStructQualifiedField
  | .externalStructOmittedField => true
  | .emptyArray | .unitTuple | .emptyStruct | .enumUnitQualified | .enumUnitOmitted
  | .enumTupleQualified | .enumTupleOmitted | .enumNamedQualified | .enumNamedOmitted => false

def allConstructorTags : List ConstructorTag := [
  .identityCall, .tupleFirst, .tupleSecond, .tupleThird, .arrayList, .arrayRepeat,
  .optionalSome, .errorOk, .errorError, .blockTail, .ifThen, .ifElse, .matchFirst, .matchSecond,
  .pointerRef, .arrayToSlice, .emptyArray, .unitTuple, .emptyStruct,
  .structTypedField, .structQualifiedField, .structOmittedField,
  .unionTypedField, .unionQualifiedField, .unionOmittedField,
  .tupleStructPayload, .aliasOmittedField, .externalStructQualifiedField,
  .externalStructOmittedField, .externalTupleStructPayload,
  .enumUnitQualified, .enumUnitOmitted, .enumTupleQualified, .enumTupleOmitted,
  .enumNamedQualified, .enumNamedOmitted
]

def constructorTags : List String := allConstructorTags.map ConstructorTag.name

def genericWrapperTags : List String :=
  (allConstructorTags.filter ConstructorTag.isGenericWrapper).map ConstructorTag.name

def Literal.tag : Literal -> String
  | .integerDefault => "integer_default"
  | .integerSuffix _ => "integer_suffix"
  | .integerNegative => "integer_negative"
  | .floatDefault => "float_default"
  | .floatSuffix _ => "float_suffix"
  | .character => "character"
  | .byteCharacter => "byte_character"
  | .string _ => "string"
  | .byteString _ => "byte_string"
  | .boolean => "boolean"
  | .null => "null"
  | .emptyArray => "array_empty"
  | .unitValue => "unit_tuple"

def literalTags : List String := [
  Literal.integerDefault.tag, (Literal.integerSuffix .u8).tag, Literal.integerNegative.tag,
  Literal.floatDefault.tag, (Literal.floatSuffix .f32).tag, Literal.character.tag,
  Literal.byteCharacter.tag, (Literal.string 0).tag, (Literal.byteString 0).tag,
  Literal.boolean.tag, Literal.null.tag, Literal.emptyArray.tag, Literal.unitValue.tag
]

def compilerCaseTags : List String := [
  "integer_default", "integer_contextual_widening", "integer_suffix", "integer_negative",
  "integer_radix", "integer_hexadecimal", "integer_octal", "integer_separators",
  "integer_min_i32", "float_default", "float_suffix", "float_exponent",
  "character", "escaped_character", "unicode_character", "byte_character", "nul_byte_character",
  "string", "adjacent_string", "byte_string", "utf8_byte_string",
  "string_auto_address_slice", "byte_string_auto_address_slice", "boolean", "global_address",
  "unit_tuple", "tuple_first", "tuple_second", "tuple_third", "tuple_of_array_and_optional",
  "array_list", "array_repeat", "array_empty", "array_empty_repeat", "array_of_tuples",
  "nested_empty_array", "optional_some", "optional_null", "error_ok", "error_error",
  "block_tail", "if_then_and_else", "match_arms", "match_block_arms", "if_pattern_value",
  "if_pattern_chain_value", "operator_context", "comparison_result", "cast_result",
  "typed_struct", "empty_struct", "qualified_struct", "omitted_struct",
  "array_of_generic_aggregates", "aliased_omitted_struct", "typed_union", "qualified_union",
  "omitted_union", "tuple_struct", "enum_unit_qualified", "enum_unit_omitted",
  "enum_tuple_qualified", "enum_tuple_omitted", "enum_named_qualified", "enum_named_omitted",
  "generic_identity", "generic_identity_of_omitted_struct", "cross_module_nested_omitted_generic",
  "generic_identity_of_omitted_union", "generic_identity_of_omitted_enum", "external_omitted_struct",
  "nested_optional_generic", "nested_array_generic", "nested_nominal_generic",
  "nested_nominal_omission", "tuple_of_omitted_aggregate_and_array", "nested_error_generic",
  "nested_optional_error_union", "branch_generic_results",
  "deep_cross_constructor_nesting", "deep_cross_module_omission",
  "integer_context_i8", "integer_suffix_i8", "negative_context_i8",
  "integer_context_i16", "integer_suffix_i16", "negative_context_i16",
  "integer_context_i32", "integer_suffix_i32", "negative_context_i32",
  "integer_context_i64", "integer_suffix_i64", "negative_context_i64",
  "integer_context_i128", "integer_suffix_i128", "negative_context_i128",
  "integer_context_isize", "integer_suffix_isize", "negative_context_isize",
  "integer_context_u8", "integer_suffix_u8", "integer_context_u16", "integer_suffix_u16",
  "integer_context_u32", "integer_suffix_u32", "integer_context_u64", "integer_suffix_u64",
  "integer_context_u128", "integer_suffix_u128", "integer_context_usize", "integer_suffix_usize",
  "float_context_f32", "float_suffix_f32", "float_context_f64", "float_suffix_f64"
]

def manifest : Lean.Json :=
  match Lean.Json.parse (include_str "value_inference_matrix.json") with
  | .ok json => json
  | .error _ => .null

def manifestNames (key : String) : List String :=
  ((manifest.getObjValAs? (List String) key).toOption.getD [])

def constructorCoverage : List String := manifestNames "constructor_coverage"

def constructorCoverageForms : List String :=
  constructorCoverage.map fun mapping => (mapping.splitOn "=").headD ""

def constructorCoverageIsWellFormed : Bool :=
  constructorCoverage.all (fun mapping =>
    (mapping.splitOn "=").length == 2 &&
      compilerCaseTags.contains ((mapping.splitOn "=").getD 1 ""))

example : manifestNames "literal_forms" = literalTags := by native_decide
example : manifestNames "constructor_forms" = constructorTags := by native_decide
example : manifestNames "generic_wrapper_forms" = genericWrapperTags := by native_decide
example : manifestNames "constructor_coverage_forms" = constructorTags := by native_decide
example : constructorCoverageForms = constructorTags := by native_decide
example : constructorCoverageIsWellFormed = true := by native_decide
example : manifestNames "ast_expression_forms" = astExpressionTags := by native_decide
example : manifestNames "compiler_expression_cases" = compilerCaseTags := by native_decide

end NiaValueInference
