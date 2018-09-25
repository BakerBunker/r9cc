use parse::{Node, NodeType};
use token::TokenType;
use util::roundup;
use {Ctype, Type, Scope, Var};

use std::sync::Mutex;
use std::collections::HashMap;
use std::mem;

// Quoted from 9cc
// > Semantics analyzer. This pass plays a few important roles as shown
// > below:
//
// > - Add types to nodes. For example, a tree that represents "1+2" is
// >   typed as INT because the result type of an addition of two
// >   integers is integer.
//
// > - Resolve variable names based on the C scope rules.
// >   Local variables are resolved to offsets from the base pointer.
// >   Global variables are resolved to their names.
//
// > - Insert nodes to make array-to-pointer conversion explicit.
// >   Recall that, in C, "array of T" is automatically converted to
// >   "pointer to T" in most contexts.
//
// > - Reject bad assignments, such as `1=2+3`.

macro_rules! matches(
    ($e:expr, $p:pat) => (
        match $e {
            $p => true,
            _ => false
        }
    )
);

fn swap(p: &mut Box<Node>, q: &mut Box<Node>) {
    mem::swap(p, q);
}

lazy_static!{
    static ref STRLABEL: Mutex<usize> = Mutex::new(0);
    static ref GLOBALS: Mutex<Vec<Var>> = Mutex::new(vec![]);
    static ref STACKSIZE: Mutex<usize> = Mutex::new(0);
}

#[derive(Debug, Clone)]
struct Env {
    vars: HashMap<String, Var>,
    next: Option<Box<Env>>,
}

impl Env {
    pub fn new(next: Option<Box<Env>>) -> Self {
        Env {
            vars: HashMap::new(),
            next,
        }
    }

    pub fn find(&self, name: &String) -> Option<&Var> {
        let mut env: &Env = self;
        loop {
            if let Some(var) = env.vars.get(name) {
                return Some(&var);
            }
            if let Some(ref next_env) = env.next {
                env = &next_env;
            } else {
                break;
            }
        }
        return None;
    }
}

fn maybe_decay(base: Node, decay: bool) -> Node {
    if !decay {
        return base;
    }

    if let Ctype::Ary(ary_of, _) = base.ty.ty.clone() {
        let mut node = Node::new(NodeType::Addr(Box::new(base)));
        node.ty = Box::new(Type::ptr_to(ary_of.clone()));
        node
    } else {
        base
    }
}

fn check_lval(node: &Node) {
    let op = &node.op;
    if matches!(op, NodeType::Lvar(_)) || matches!(op, NodeType::Gvar(_, _, _)) ||
        matches!(op, NodeType::Deref(_)) || matches!(op, NodeType::Dot(_, _, _))
    {
        return;
    }
    panic!("not an lvalue: {:?}", node.op);
}

fn walk(mut node: Node, env: &mut Env, decay: bool) -> Node {
    use self::NodeType::*;
    let op = node.op.clone();
    match op {
        Num(_) => (),
        Str(data, len) => {
            let name = format!(".L.str{}", *STRLABEL.lock().unwrap());
            *STRLABEL.lock().unwrap() += 1;
            let var = Var::new_global(node.ty.clone(), name, data, len, false);
            let name = var.name.clone();
            GLOBALS.lock().unwrap().push(var);

            let mut ret = Node::new(NodeType::Gvar(name, "".into(), len));
            ret.ty = node.ty;
            return maybe_decay(ret, decay);
        }
        Ident(ref name) => {
            if let Some(var) = env.find(name) {
                match var.scope {
                    Scope::Local(offset) => {
                        let mut ret = Node::new(NodeType::Lvar(Scope::Local(offset)));
                        ret.ty = var.ty.clone();
                        return maybe_decay(ret, decay);
                    }
                    Scope::Global(ref data, len, _) => {
                        let mut ret =
                            Node::new(NodeType::Gvar(var.name.clone(), data.clone(), len));
                        ret.ty = var.ty.clone();
                        return maybe_decay(ret, decay);
                    }
                }
            } else {
                panic!("undefined variable: {}", name);
            }
        }
        Vardef(name, init_may, _) => {
            let stacksize = *STACKSIZE.lock().unwrap();
            *STACKSIZE.lock().unwrap() = roundup(stacksize, node.ty.align);
            *STACKSIZE.lock().unwrap() += node.ty.size;
            let offset = *STACKSIZE.lock().unwrap();

            env.vars.insert(
                name.clone(),
                Var::new(node.ty.clone(), name.clone(), Scope::Local(offset)),
            );

            let mut init = None;
            if let Some(mut init2) = init_may {
                init = Some(Box::new(walk(*init2, env, true)));
            }
            node.op = Vardef(name, init, Scope::Local(offset));
        }
        If(mut cond, mut then, els_may) => {
            cond = Box::new(walk(*cond, env, true));
            then = Box::new(walk(*then, env, true));
            let mut new_els = None;
            if let Some(mut els) = els_may {
                new_els = Some(Box::new(walk(*els, env, true)));
            }
            node.op = If(cond, then, new_els);
        }
        Ternary(mut cond, mut then, mut els) => {
            cond = Box::new(walk(*cond, env, true));
            then = Box::new(walk(*then, env, true));
            els = Box::new(walk(*els, env, true));
            node.ty = then.ty.clone();
            node.op = Ternary(cond, then, els);
        }
        For(init, cond, inc, body) => {
            node.op = For(
                Box::new(walk(*init, env, true)),
                Box::new(walk(*cond, env, true)),
                Box::new(walk(*inc, env, true)),
                Box::new(walk(*body, env, true)),
            );
        }
        DoWhile(body, cond) => {
            node.op = DoWhile(
                Box::new(walk(*body, env, true)),
                Box::new(walk(*cond, env, true)),
            );
        }
        Dot(mut expr, name, _) => {
            expr = Box::new(walk(*expr, env, true));
            let offset;
            if let Ctype::Struct(ref members) = expr.ty.ty {
                let m_may = members.into_iter().find(|m| {
                    if let NodeType::Vardef(ref m_name, _, _) = m.op {
                        if m_name != &name {
                            return false;
                        }
                        return true;
                    }
                    false
                });

                if let Some(m) = m_may {
                    if let NodeType::Vardef(_, _, Scope::Local(offset2)) = m.op {
                        node.ty = m.ty.clone();
                        offset = offset2;
                    } else {
                        unreachable!()
                    }
                } else {
                    panic!("member missing: {}", name);
                }
            } else {
                panic!("struct expected before '.'");
            }

            node.op = NodeType::Dot(expr, name, offset);
            return maybe_decay(node, decay);
        }
        BinOp(token_type, mut lhs, mut rhs) => {
            match token_type {
                TokenType::Plus | TokenType::Minus => {
                    lhs = Box::new(walk(*lhs, env, true));
                    rhs = Box::new(walk(*rhs, env, true));

                    if matches!(rhs.ty.ty , Ctype::Ptr(_)) {
                        swap(&mut lhs, &mut rhs);
                    }
                    if matches!(rhs.ty.ty , Ctype::Ptr(_)) {
                        panic!("'pointer {:?} pointer' is not defined", node.op)
                    }

                    node.op = BinOp(token_type, lhs.clone(), rhs);
                    node.ty = lhs.ty;
                }
                TokenType::Equal => {
                    lhs = Box::new(walk(*lhs, env, false));
                    check_lval(&*lhs);
                    node.op = BinOp(token_type, lhs.clone(), Box::new(walk(*rhs, env, true)));
                    node.ty = lhs.ty;
                }
                _ => {
                    lhs = Box::new(walk(*lhs, env, true));
                    rhs = Box::new(walk(*rhs, env, true));
                    node.op = BinOp(token_type, lhs.clone(), rhs);
                    node.ty = lhs.ty;
                }
            }
        }
        PreInc(mut expr) => {
            expr = Box::new(walk(*expr, env, true));
            node.op = PreInc(expr);
        }
        PreDec(mut expr) => {
            expr = Box::new(walk(*expr, env, true));
            node.op = PreDec(expr);
        }
        PostInc(mut expr) => {
            expr = Box::new(walk(*expr, env, true));
            node.op = PostInc(expr);
        }
        PostDec(mut expr) => {
            expr = Box::new(walk(*expr, env, true));
            node.op = PostDec(expr);
        }
        Neg(mut expr) => {
            expr = Box::new(walk(*expr, env, true));
            node.op = Neg(expr);
        }
        Exclamation(mut expr) => {
            expr = Box::new(walk(*expr, env, true));
            node.op = Exclamation(expr);
        }
        Addr(mut expr) => {
            expr = Box::new(walk(*expr, env, true));
            check_lval(&*expr);
            node.ty = Box::new(Type::ptr_to(expr.ty.clone()));
            node.op = Addr(expr);
        }
        Deref(mut expr) => {
            expr = Box::new(walk(*expr, env, true));
            match expr.ty.ty {
                Ctype::Ptr(ref ptr_to) => node.ty = ptr_to.clone(),
                Ctype::Void => panic!("cannot dereference void opinter"),
                _ => panic!("operand must be a pointer"),
            }
            node.op = Deref(expr);
        }
        Return(expr) => node.op = Return(Box::new(walk(*expr, env, true))),
        ExprStmt(expr) => node.op = ExprStmt(Box::new(walk(*expr, env, true))),
        Sizeof(mut expr) => {
            expr = Box::new(walk(*expr, env, false));
            node = Node::int_ty(expr.ty.size as i32)
        }
        Alignof(mut expr) => {
            expr = Box::new(walk(*expr, env, false));
            node = Node::int_ty(expr.ty.align as i32)
        }
        Call(name, mut args) => {
            args = args.into_iter().map(|arg| walk(arg, env, true)).collect();
            node.op = Call(name, args);
        }
        Func(name, mut args, body, stacksize) => {
            args = args.into_iter().map(|arg| walk(arg, env, true)).collect();
            node.op = Func(name, args, Box::new(walk(*body, env, true)), stacksize);
        }
        CompStmt(mut stmts) => {
            let mut new_env = Env::new(Some(Box::new(env.clone())));
            stmts = stmts
                .into_iter()
                .map(|stmt| walk(stmt, &mut new_env, true))
                .collect();
            node.op = CompStmt(stmts);
        }
        VecStmt(mut stmts) => {
            stmts = stmts
                .into_iter()
                .map(|stmt| walk(stmt, env, true))
                .collect();
            node.op = VecStmt(stmts);
        }
        StmtExpr(body) => {
            node.op = StmtExpr(Box::new(walk(*body, env, true)));
            node.ty = Box::new(Type::int_ty())
        }
        Null => (),
        _ => panic!("unknown node type"),
    };
    node
}

pub fn sema(nodes: Vec<Node>) -> (Vec<Node>, Vec<Var>) {
    let mut new_nodes = vec![];
    let mut topenv = Env::new(None);

    for mut node in nodes {
        if let NodeType::Vardef(name, _, Scope::Global(data, len, is_extern)) = node.op {
            let var = Var::new_global(node.ty, name.clone(), data, len, is_extern);
            GLOBALS.lock().unwrap().push(var.clone());
            topenv.vars.insert(name, var);
            continue;
        }

        assert!(matches!(node.op, NodeType::Func(_, _, _, _)));
        let mut new = walk(node, &mut topenv, true);
        if let NodeType::Func(name, args, body, _) = new.op {
            new.op = NodeType::Func(name.clone(), args, body, *STACKSIZE.lock().unwrap());
            *STACKSIZE.lock().unwrap() = 0;
            new_nodes.push(new);
        }
    }
    (new_nodes, GLOBALS.lock().unwrap().clone())
}
