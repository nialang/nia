// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

#[test]
fn parses_trait_impl_where_and_self_type() {
    let (module, errors) = parse_module(
        r#"
trait Show {
    fn show(&self) i32;
    fn clone_self(&self) Self {
        self.*
    }
}

struct Box[T] where T: Show {
    value: T,
}

extend Box[i32] : Show where i32: Show {
    fn show(&self) i32 {
        self.value
    }
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    assert!(matches!(module.items[0].kind, ItemKind::Trait(_)));
    let ItemKind::Struct(item_struct) = &module.items[1].kind else {
        panic!("expected struct");
    };
    assert_eq!(item_struct.where_clause.predicates.len(), 1);
    let ItemKind::Extend(extend) = &module.items[2].kind else {
        panic!("expected extend");
    };
    assert!(extend.trait_ref.is_some());
    assert_eq!(extend.where_clause.predicates.len(), 1);
}

#[test]
fn parses_supertraits_with_plus_bounds() {
    let (module, errors) = parse_module(
        r#"
trait Same {
    fn eq(&self, other: &Self) bool;
}

trait Show {
    fn show(&self) i32;
}

trait Ranked : Same + Show
where Self: Same {
    fn lt(&self, other: &Self) bool;
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Trait(item_trait) = &module.items[2].kind else {
        panic!("expected trait");
    };
    assert_eq!(item_trait.supertraits.len(), 2);
    assert_eq!(item_trait.where_clause.predicates.len(), 1);
}
