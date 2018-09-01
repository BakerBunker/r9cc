// Compile AST to intermediate code that has infinite number of registers.
// Base pointer is always assigned to r0.

use parse::{Node, NodeType, Ctype};
use token::TokenType;
use sema::size_of;

use std::sync::Mutex;
use std::fmt;

lazy_static!{
    static ref NREG: Mutex<usize> = Mutex::new(1);
    static ref NLABEL: Mutex<usize> = Mutex::new(0);
    static ref CODE: Mutex<Vec<IR>> = Mutex::new(vec![]);
}

fn add(op: IROp, lhs: Option<usize>, rhs: Option<usize>) {
    CODE.lock().unwrap().push(IR::new(op, lhs, rhs));
}

#[derive(Clone, Debug)]
pub enum IRType {
    Noarg,
    Reg,
    Imm,
    Jmp,
    Label,
    RegReg,
    RegImm,
    ImmImm,
    RegLabel,
    Call,
}

#[derive(Clone, Debug)]
pub struct IRInfo {
    name: &'static str,
    pub ty: IRType,
}

impl IRInfo {
    pub fn new(name: &'static str, ty: IRType) -> Self {
        IRInfo { name: name, ty: ty }
    }
}

pub fn dump_ir(fns: &Vec<Function>) {
    for f in fns {
        print!("{}(): \n", f.name);
        for ir in &f.ir {
            print!("{}\n", ir);
        }
    }
}

#[derive(Clone, Debug)]
pub struct Function {
    pub name: String,
    pub ir: Vec<IR>,
    pub stacksize: usize,
}

impl Function {
    fn new(name: String, ir: Vec<IR>, stacksize: usize) -> Self {
        Function {
            name: name,
            ir: ir,
            stacksize: stacksize,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum IROp {
    Add,
    Sub,
    Mul,
    Div,
    Imm,
    SubImm,
    Mov,
    Return,
    Call(String, usize, [usize; 6]),
    Label,
    LT,
    Jmp,
    Unless,
    Load32,
    Load64,
    Store32,
    Store64,
    Store32Arg,
    Store64Arg,
    Kill,
    Nop,
}

impl<'a> From<&'a IROp> for IRInfo {
    fn from(op: &'a IROp) -> IRInfo {
        use self::IROp::*;
        match op {
            Add => IRInfo::new("ADD", IRType::RegReg),
            Call(_, _, _) => IRInfo::new("CALL", IRType::Call),
            Div => IRInfo::new("DIV", IRType::RegReg),
            Imm => IRInfo::new("MOV", IRType::RegImm),
            Jmp => IRInfo::new("JMP", IRType::Jmp),
            Kill => IRInfo::new("KILL", IRType::Reg),
            Label => IRInfo::new("", IRType::Label),
            LT => IRInfo::new("LT", IRType::RegReg),
            Load32 => IRInfo::new("LOAD32", IRType::RegReg),
            Load64 => IRInfo::new("LOAD64", IRType::RegReg),
            Mov => IRInfo::new("MOV", IRType::RegReg),
            Mul => IRInfo::new("MUL", IRType::RegReg),
            Nop => IRInfo::new("NOP", IRType::Noarg),
            Return => IRInfo::new("RET", IRType::Reg),
            Store32 => IRInfo::new("STORE32", IRType::RegReg),
            Store64 => IRInfo::new("STORE64", IRType::RegReg),
            Store32Arg => IRInfo::new("STORE32_ARG", IRType::ImmImm),
            Store64Arg => IRInfo::new("STORE64_ARG", IRType::ImmImm),
            Sub => IRInfo::new("SUB", IRType::RegReg),
            SubImm => IRInfo::new("SUB", IRType::RegImm),
            Unless => IRInfo::new("UNLESS", IRType::RegLabel),
        }
    }
}

impl fmt::Display for IR {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        use self::IRType::*;

        let info = &IRInfo::from(&self.op);

        let lhs = self.lhs.unwrap();
        match info.ty {
            Label => write!(f, ".L{}=>", lhs),
            Imm => write!(f, "  {} {}", info.name, lhs),
            Reg => write!(f, "  {} r{}", info.name, lhs),
            Jmp => write!(f, "  {} .L{}", info.name, lhs),
            RegReg => write!(f, "  {} r{}, r{}", info.name, lhs, self.rhs.unwrap()),
            RegImm => write!(f, "  {} r{}, {}", info.name, lhs, self.rhs.unwrap()),
            ImmImm => write!(f, "  {} {}, {}", info.name, lhs, self.rhs.unwrap()),
            RegLabel => write!(f, "  {} r{}, .L{}", info.name, lhs, self.rhs.unwrap()),
            Call => {
                match self.op {
                    IROp::Call(ref name, nargs, args) => {
                        let mut sb: String = format!("  r{} = {}(", lhs, name);
                        for i in 0..nargs {
                            sb.push_str(&format!(", r{}", args[i]));
                        }
                        sb.push_str(")");
                        write!(f, "{}", sb)
                    }
                    _ => unreachable!(),
                }
            }
            Noarg => write!(f, "  {}", info.name),
        }
    }
}

impl From<NodeType> for IROp {
    fn from(node_type: NodeType) -> Self {
        match node_type {
            NodeType::BinOp(op, _, _) => Self::from(op),
            e => panic!("cannot convert: {:?}", e),
        }
    }
}

impl From<TokenType> for IROp {
    fn from(token_type: TokenType) -> Self {
        match token_type {
            TokenType::Plus => IROp::Add,
            TokenType::Minus => IROp::Sub,
            TokenType::Mul => IROp::Mul,
            TokenType::Div => IROp::Div,
            TokenType::LeftAngleBracket |
            TokenType::RightAngleBracket => IROp::LT,
            e => panic!("cannot convert: {:?}", e),
        }
    }
}

#[derive(Debug, Clone)]
pub struct IR {
    pub op: IROp,
    pub lhs: Option<usize>,
    pub rhs: Option<usize>,
}

impl IR {
    fn new(op: IROp, lhs: Option<usize>, rhs: Option<usize>) -> Self {
        Self {
            op: op,
            lhs: lhs,
            rhs: rhs,
        }
    }
}

fn kill(r: Option<usize>) {
    add(IROp::Kill, r, None);
}

fn label(x: Option<usize>) {
    add(IROp::Label, x, None);
}

fn gen_lval(node: Node) -> Option<usize> {
    match node.op {
        NodeType::Deref(expr) => gen_expr(*expr),
        NodeType::Lvar => {
            let r = Some(*NREG.lock().unwrap());
            *NREG.lock().unwrap() += 1;
            add(IROp::Mov, r, Some(0));
            add(IROp::SubImm, r, Some(node.offset));
            return r;
        }
        _ => panic!("not an lvalue: {:?}", node.op),
    }
}

fn gen_binop(ty: IROp, lhs: Box<Node>, rhs: Box<Node>) -> Option<usize> {
    let r1 = gen_expr(*lhs);
    let r2 = gen_expr(*rhs);

    add(ty, r1, r2);
    kill(r2);
    return r1;
}

fn gen_expr(node: Node) -> Option<usize> {
    match node.op {
        NodeType::Num(val) => {
            let r = Some(*NREG.lock().unwrap());
            *NREG.lock().unwrap() += 1;
            add(IROp::Imm, r, Some(val as usize));
            return r;
        }
        NodeType::Logand(lhs, rhs) => {
            let x = Some(*NLABEL.lock().unwrap());
            *NLABEL.lock().unwrap() += 1;

            let r1 = gen_expr(*lhs);
            add(IROp::Unless, r1, x);
            let r2 = gen_expr(*rhs);
            add(IROp::Mov, r1, r2);
            kill(r2);
            add(IROp::Unless, r1, x);
            add(IROp::Imm, r1, Some(1));
            label(x);
            return r1;
        }
        NodeType::Logor(lhs, rhs) => {
            let x = Some(*NLABEL.lock().unwrap());
            *NLABEL.lock().unwrap() += 1;
            let y = Some(*NLABEL.lock().unwrap());
            *NLABEL.lock().unwrap() += 1;

            let r1 = gen_expr(*lhs);
            add(IROp::Unless, r1, x);
            add(IROp::Imm, r1, Some(1));
            add(IROp::Jmp, y, None);
            label(x);

            let r2 = gen_expr(*rhs);
            add(IROp::Mov, r1, r2);
            kill(r2);
            add(IROp::Unless, r1, y);
            add(IROp::Imm, r1, Some(1));
            label(y);
            return r1;
        }
        NodeType::Lvar => {
            let ty = node.ty.ty.clone();
            let r = gen_lval(node);
            match ty {
                Ctype::Ptr(_) => add(IROp::Load64, r, r),
                _ => add(IROp::Load32, r, r),
            }
            return r;
        }
        NodeType::Call(name, args) => {
            let mut args_ir: [usize; 6] = [0; 6];
            for i in 0..args.len() {
                args_ir[i] = gen_expr(args[i].clone()).unwrap();
            }

            let r = Some(*NREG.lock().unwrap());
            *NREG.lock().unwrap() += 1;

            add(IROp::Call(name, args.len(), args_ir), r, None);

            for i in 0..args.len() {
                kill(Some(args_ir[i]));
            }
            return r;
        }
        NodeType::Addr(expr) => gen_lval(*expr),
        NodeType::Deref(expr) => {
            let r = gen_expr(*expr);
            add(IROp::Load64, r, r);
            return r;
        }
        NodeType::BinOp(op, lhs, rhs) => {
            match op {
                TokenType::Equal => {
                    let rhs = gen_expr(*rhs);
                    let lhs = gen_lval(*lhs);
                    match node.ty.ty {
                        Ctype::Ptr(_) => add(IROp::Store64, lhs, rhs),
                        _ => add(IROp::Store32, lhs, rhs),
                    }
                    kill(rhs);
                    return lhs;
                }
                TokenType::Plus | TokenType::Minus => {
                    let insn = IROp::from(op);
                    if let Ctype::Ptr(ref ptr_of) = lhs.ty.ty.clone() {
                        let rhs = gen_expr(*rhs);
                        let r = Some(*NREG.lock().unwrap());
                        *NREG.lock().unwrap() += 1;
                        add(IROp::Imm, r, Some(size_of(ptr_of)));
                        add(IROp::Mul, rhs, r);
                        kill(r);

                        let lhs = gen_expr(*lhs);
                        add(insn, lhs, rhs);
                        kill(rhs);
                        lhs
                    } else {
                        gen_binop(insn, lhs, rhs)
                    }
                }
                _ => gen_binop(IROp::from(op), lhs, rhs),
            }
        }
        e => unreachable!("{:?}", e),
    }
}

fn gen_stmt(node: Node) {
    match node.op {
        NodeType::Vardef(_, init_may) => {
            if let Some(init) = init_may {
                let rhs = gen_expr(*init);
                let lhs = Some(*NREG.lock().unwrap());
                *NREG.lock().unwrap() += 1;
                add(IROp::Mov, lhs, Some(0));
                add(IROp::SubImm, lhs, Some(node.offset));
                match node.ty.ty {
                    Ctype::Ptr(_) => add(IROp::Store64, lhs, rhs),
                    _ => add(IROp::Store32, lhs, rhs),
                }
                kill(lhs);
                kill(rhs);
            }
            return;
        }
        NodeType::If(cond, then, els_may) => {
            if let Some(els) = els_may {
                let x = Some(*NLABEL.lock().unwrap());
                *NLABEL.lock().unwrap() += 1;
                let y = Some(*NLABEL.lock().unwrap());
                *NLABEL.lock().unwrap() += 1;
                let r = gen_expr(*cond.clone());
                add(IROp::Unless, r, x);
                kill(r);
                gen_stmt(*then.clone());
                add(IROp::Jmp, y, None);
                label(x);
                gen_stmt(*els);
                label(y);
            }

            let x = Some(*NLABEL.lock().unwrap());
            *NLABEL.lock().unwrap() += 1;
            let r = gen_expr(*cond);
            add(IROp::Unless, r, x);
            kill(r);
            gen_stmt(*then);
            label(x);
        }
        NodeType::For(init, cond, inc, body) => {
            let x = Some(*NLABEL.lock().unwrap());
            *NLABEL.lock().unwrap() += 1;
            let y = Some(*NLABEL.lock().unwrap());
            *NLABEL.lock().unwrap() += 1;

            gen_stmt(*init);
            label(x);
            let r2 = gen_expr(*cond);
            add(IROp::Unless, r2, y);
            kill(r2);
            gen_stmt(*body);
            let r3 = gen_expr(*inc);
            kill(r3);
            add(IROp::Jmp, x, None);
            label(y);
        }
        NodeType::Return(expr) => {
            let r = gen_expr(*expr);
            add(IROp::Return, r, None);
            kill(r);
        }
        NodeType::ExprStmt(expr) => {
            let r = gen_expr(*expr);
            kill(r);
        }
        NodeType::CompStmt(stmts) => {
            for n in stmts {
                gen_stmt(n);
            }
        }
        e => panic!("unknown node: {:?}", e),
    }
}

pub fn gen_ir(nodes: Vec<Node>) -> Vec<Function> {
    let mut v = vec![];
    for node in nodes {
        match node.op {
            NodeType::Func(name, args, body) => {
                *CODE.lock().unwrap() = vec![];
                *NREG.lock().unwrap() = 1;

                for i in 0..args.len() {
                    let arg = &args[i];
                    let op = match arg.ty.ty {
                        Ctype::Ptr(_) => IROp::Store64Arg,
                        _ => IROp::Store32Arg,
                    };
                    add(op, Some(arg.offset), Some(i));
                }
                gen_stmt(*body);

                v.push(Function::new(
                    name,
                    CODE.lock().unwrap().clone(),
                    node.stacksize,
                ));
            }
            _ => panic!("parse error."),
        }
    }
    v
}