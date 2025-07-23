//! Parser for Lisp expressions.
//! Converts tokens from the lexer into an Abstract Syntax Tree (AST).

use crate::ast::Expr;
use crate::lexer::Token;
use crate::symbol::Symbol;
use crate::var::Var;

/// Parser error types
#[derive(Debug, Clone)]
pub enum ParseError {
    /// Unexpected token encountered
    UnexpectedToken { expected: String, found: Token },
    /// Unexpected end of input
    UnexpectedEof { expected: String },
    /// Invalid special form syntax
    InvalidSpecialForm { form: String, reason: String },
    /// Generic parse error
    Generic(String),
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::UnexpectedToken { expected, found } => {
                write!(f, "Expected {}, but found {}", expected, found)
            }
            ParseError::UnexpectedEof { expected } => {
                write!(f, "Unexpected end of input, expected {}", expected)
            }
            ParseError::InvalidSpecialForm { form, reason } => {
                write!(f, "Invalid {} form: {}", form, reason)
            }
            ParseError::Generic(msg) => write!(f, "{}", msg),
        }
    }
}

impl std::error::Error for ParseError {}

/// Recursive descent parser for Lisp expressions
pub struct Parser {
    tokens: Vec<Token>,
    position: usize,
}

impl Parser {
    /// Create a new parser with the given tokens
    pub fn new(tokens: Vec<Token>) -> Self {
        Self {
            tokens,
            position: 0,
        }
    }
    
    /// Get the current token without consuming it
    fn current_token(&self) -> &Token {
        self.tokens.get(self.position).unwrap_or(&Token::Eof)
    }
    
    /// Advance to the next token and return the previous one
    fn advance(&mut self) -> Token {
        let token = self.current_token().clone();
        if self.position < self.tokens.len() {
            self.position += 1;
        }
        token
    }
    
    /// Check if we're at the end of input
    fn is_at_end(&self) -> bool {
        matches!(self.current_token(), Token::Eof)
    }
    
    /// Consume a token if it matches the expected type, otherwise return error
    fn consume(&mut self, expected: Token, message: &str) -> Result<Token, ParseError> {
        if std::mem::discriminant(self.current_token()) == std::mem::discriminant(&expected) {
            Ok(self.advance())
        } else {
            Err(ParseError::UnexpectedToken {
                expected: message.to_string(),
                found: self.current_token().clone(),
            })
        }
    }
    
    /// Parse a single expression
    pub fn parse_expr(&mut self) -> Result<Expr, ParseError> {
        match self.current_token() {
            Token::LeftParen => self.parse_list(),
            Token::Integer(n) => {
                let value = *n;
                self.advance();
                Ok(Expr::Literal(Var::int(value)))
            }
            Token::Float(n) => {
                let value = *n;
                self.advance();
                Ok(Expr::Literal(Var::float(value)))
            }
            Token::Symbol(s) => {
                let sym = Symbol::mk(s);
                self.advance();
                Ok(Expr::Variable(sym))
            }
            Token::Keyword(s) => {
                let keyword = s.clone();
                self.advance();
                // For now, treat keywords as literal symbols
                // TODO: Add proper keyword support to Var and AST
                Ok(Expr::Literal(Var::string(&format!(":{}", keyword))))
            }
            Token::String(s) => {
                let value = s.clone();
                self.advance();
                Ok(Expr::Literal(Var::string(&value)))
            }
            Token::RightParen => {
                Err(ParseError::UnexpectedToken {
                    expected: "expression".to_string(),
                    found: self.current_token().clone(),
                })
            }
            Token::Eof => {
                Err(ParseError::UnexpectedEof {
                    expected: "expression".to_string(),
                })
            }
        }
    }
    
    /// Parse a list expression (function call or special form)
    fn parse_list(&mut self) -> Result<Expr, ParseError> {
        self.consume(Token::LeftParen, "(")?;
        
        // Handle empty list
        if matches!(self.current_token(), Token::RightParen) {
            self.advance();
            return Ok(Expr::Literal(Var::list(&[])));
        }
        
        // Parse the first element to see if it's a special form
        let first_expr = self.parse_expr()?;
        
        // Check if this is a special form
        if let Expr::Variable(sym) = &first_expr {
            match sym.as_string().as_str() {
                "let" => return self.parse_let(),
                "if" => return self.parse_if(),
                "lambda" => return self.parse_lambda(),
                _ => {
                    // Regular function call
                    let mut args = Vec::new();
                    while !matches!(self.current_token(), Token::RightParen | Token::Eof) {
                        args.push(self.parse_expr()?);
                    }
                    
                    self.consume(Token::RightParen, ")")?;
                    return Ok(Expr::Call {
                        func: Box::new(first_expr),
                        args,
                    });
                }
            }
        }
        
        // If first element wasn't a special form symbol, treat as regular function call
        let mut args = Vec::new();
        while !matches!(self.current_token(), Token::RightParen | Token::Eof) {
            args.push(self.parse_expr()?);
        }
        
        self.consume(Token::RightParen, ")")?;
        Ok(Expr::Call {
            func: Box::new(first_expr),
            args,
        })
    }
    
    /// Parse a let binding: (let ((var1 val1) (var2 val2) ...) body)
    fn parse_let(&mut self) -> Result<Expr, ParseError> {
        // We already consumed "let", now expect the bindings list
        self.consume(Token::LeftParen, "(")?;
        
        let mut bindings = Vec::new();
        
        // Parse each binding
        while !matches!(self.current_token(), Token::RightParen | Token::Eof) {
            // Each binding is (var value)
            self.consume(Token::LeftParen, "(")?;
            
            // Get the variable name
            let var_expr = self.parse_expr()?;
            let var_symbol = match var_expr {
                Expr::Variable(sym) => sym,
                _ => {
                    return Err(ParseError::InvalidSpecialForm {
                        form: "let".to_string(),
                        reason: "binding variable must be a symbol".to_string(),
                    });
                }
            };
            
            // Get the value expression
            let value_expr = self.parse_expr()?;
            
            self.consume(Token::RightParen, ")")?;
            
            bindings.push((var_symbol, value_expr));
        }
        
        self.consume(Token::RightParen, ")")?;
        
        // Parse the body expression
        let body = self.parse_expr()?;
        
        self.consume(Token::RightParen, ")")?;
        
        Ok(Expr::Let {
            bindings,
            body: Box::new(body),
        })
    }
    
    /// Parse an if expression: (if condition then-expr else-expr)
    fn parse_if(&mut self) -> Result<Expr, ParseError> {
        // We already consumed "if"
        let condition = self.parse_expr()?;
        let then_expr = self.parse_expr()?;
        let else_expr = self.parse_expr()?;
        
        self.consume(Token::RightParen, ")")?;
        
        Ok(Expr::If {
            condition: Box::new(condition),
            then_expr: Box::new(then_expr),
            else_expr: Box::new(else_expr),
        })
    }
    
    /// Parse a lambda expression: (lambda (param1 param2 ...) body)
    fn parse_lambda(&mut self) -> Result<Expr, ParseError> {
        // We already consumed "lambda", now expect parameter list
        self.consume(Token::LeftParen, "(")?;
        
        let mut params = Vec::new();
        
        // Parse parameters
        while !matches!(self.current_token(), Token::RightParen | Token::Eof) {
            let param_expr = self.parse_expr()?;
            let param_symbol = match param_expr {
                Expr::Variable(sym) => sym,
                _ => {
                    return Err(ParseError::InvalidSpecialForm {
                        form: "lambda".to_string(),
                        reason: "parameter must be a symbol".to_string(),
                    });
                }
            };
            params.push(param_symbol);
        }
        
        self.consume(Token::RightParen, ")")?;
        
        // Parse the body
        let body = self.parse_expr()?;
        
        self.consume(Token::RightParen, ")")?;
        
        Ok(Expr::Lambda {
            params,
            body: Box::new(body),
        })
    }
    
    /// Parse a complete program (multiple expressions)
    pub fn parse(&mut self) -> Result<Vec<Expr>, ParseError> {
        let mut expressions = Vec::new();
        
        while !self.is_at_end() {
            expressions.push(self.parse_expr()?);
        }
        
        Ok(expressions)
    }
}

/// Convenience function to parse a string into expressions
pub fn parse_string(input: &str) -> Result<Vec<Expr>, Box<dyn std::error::Error>> {
    let mut lexer = crate::lexer::Lexer::new(input);
    let tokens = lexer.tokenize()?;
    let mut parser = Parser::new(tokens);
    Ok(parser.parse()?)
}

/// Convenience function to parse a single expression from a string
pub fn parse_expr_string(input: &str) -> Result<Expr, Box<dyn std::error::Error>> {
    let expressions = parse_string(input)?;
    match expressions.len() {
        0 => Err("No expressions found".into()),
        1 => Ok(expressions.into_iter().next().unwrap()),
        _ => Err("Multiple expressions found, expected one".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_parse_atoms() {
        // Test numbers
        let expr = parse_expr_string("42").unwrap();
        assert_eq!(expr, Expr::Literal(Var::int(42)));
        
        let expr = parse_expr_string("3.14").unwrap();
        assert_eq!(expr, Expr::Literal(Var::float(3.14)));
        
        // Test symbols
        let expr = parse_expr_string("foo").unwrap();
        assert_eq!(expr, Expr::Variable(Symbol::mk("foo")));
        
        // Test strings
        let expr = parse_expr_string(r#""hello""#).unwrap();
        assert_eq!(expr, Expr::Literal(Var::string("hello")));
    }
    
    #[test]
    fn test_parse_function_call() {
        let expr = parse_expr_string("(+ 1 2)").unwrap();
        
        match expr {
            Expr::Call { func, args } => {
                assert_eq!(*func, Expr::Variable(Symbol::mk("+")));
                assert_eq!(args.len(), 2);
                assert_eq!(args[0], Expr::Literal(Var::int(1)));
                assert_eq!(args[1], Expr::Literal(Var::int(2)));
            }
            _ => panic!("Expected function call"),
        }
    }
    
    #[test]
    fn test_parse_nested_calls() {
        let expr = parse_expr_string("(+ (* 2 3) 4)").unwrap();
        
        match expr {
            Expr::Call { func, args } => {
                assert_eq!(*func, Expr::Variable(Symbol::mk("+")));
                assert_eq!(args.len(), 2);
                
                // First arg should be (* 2 3)
                match &args[0] {
                    Expr::Call { func, args } => {
                        assert_eq!(**func, Expr::Variable(Symbol::mk("*")));
                        assert_eq!(args[0], Expr::Literal(Var::int(2)));
                        assert_eq!(args[1], Expr::Literal(Var::int(3)));
                    }
                    _ => panic!("Expected nested call"),
                }
                
                assert_eq!(args[1], Expr::Literal(Var::int(4)));
            }
            _ => panic!("Expected function call"),
        }
    }
    
    #[test]
    fn test_parse_let_binding() {
        let expr = parse_expr_string("(let ((x 5) (y 3)) (+ x y))").unwrap();
        
        match expr {
            Expr::Let { bindings, body } => {
                assert_eq!(bindings.len(), 2);
                
                assert_eq!(bindings[0].0, Symbol::mk("x"));
                assert_eq!(bindings[0].1, Expr::Literal(Var::int(5)));
                
                assert_eq!(bindings[1].0, Symbol::mk("y"));
                assert_eq!(bindings[1].1, Expr::Literal(Var::int(3)));
                
                // Body should be (+ x y)
                match body.as_ref() {
                    Expr::Call { func, args } => {
                        assert_eq!(**func, Expr::Variable(Symbol::mk("+")));
                        assert_eq!(args[0], Expr::Variable(Symbol::mk("x")));
                        assert_eq!(args[1], Expr::Variable(Symbol::mk("y")));
                    }
                    _ => panic!("Expected function call in body"),
                }
            }
            _ => panic!("Expected let binding"),
        }
    }
    
    #[test]
    fn test_parse_if_expression() {
        let expr = parse_expr_string("(if (> x 0) x (- x))").unwrap();
        
        match expr {
            Expr::If { condition, then_expr, else_expr } => {
                // Condition should be (> x 0)
                match condition.as_ref() {
                    Expr::Call { func, args } => {
                        assert_eq!(**func, Expr::Variable(Symbol::mk(">")));
                        assert_eq!(args[0], Expr::Variable(Symbol::mk("x")));
                        assert_eq!(args[1], Expr::Literal(Var::int(0)));
                    }
                    _ => panic!("Expected function call in condition"),
                }
                
                assert_eq!(then_expr.as_ref(), &Expr::Variable(Symbol::mk("x")));
                
                // Else should be (- x)
                match else_expr.as_ref() {
                    Expr::Call { func, args } => {
                        assert_eq!(**func, Expr::Variable(Symbol::mk("-")));
                        assert_eq!(args[0], Expr::Variable(Symbol::mk("x")));
                    }
                    _ => panic!("Expected function call in else"),
                }
            }
            _ => panic!("Expected if expression"),
        }
    }
    
    #[test]
    fn test_parse_lambda() {
        let expr = parse_expr_string("(lambda (x y) (+ x y))").unwrap();
        
        match expr {
            Expr::Lambda { params, body } => {
                assert_eq!(params.len(), 2);
                assert_eq!(params[0], Symbol::mk("x"));
                assert_eq!(params[1], Symbol::mk("y"));
                
                // Body should be (+ x y)
                match body.as_ref() {
                    Expr::Call { func, args } => {
                        assert_eq!(**func, Expr::Variable(Symbol::mk("+")));
                        assert_eq!(args[0], Expr::Variable(Symbol::mk("x")));
                        assert_eq!(args[1], Expr::Variable(Symbol::mk("y")));
                    }
                    _ => panic!("Expected function call in body"),
                }
            }
            _ => panic!("Expected lambda expression"),
        }
    }
    
    #[test]
    fn test_parse_keywords() {
        let expr = parse_expr_string(":foo").unwrap();
        assert_eq!(expr, Expr::Literal(Var::string(":foo")));
    }
    
    #[test]
    fn test_multiple_expressions() {
        let exprs = parse_string("42 (+ 1 2) foo").unwrap();
        
        assert_eq!(exprs.len(), 3);
        assert_eq!(exprs[0], Expr::Literal(Var::int(42)));
        assert_eq!(exprs[2], Expr::Variable(Symbol::mk("foo")));
    }
    
    #[test]
    fn test_empty_list() {
        let expr = parse_expr_string("()").unwrap();
        assert_eq!(expr, Expr::Literal(Var::list(&[])));
    }
    
    #[test]
    fn test_error_unexpected_token() {
        let result = parse_expr_string(")");
        assert!(result.is_err());
    }
    
    #[test]
    fn test_error_unterminated_list() {
        let result = parse_expr_string("(+ 1 2");
        assert!(result.is_err());
    }
    
    #[test]
    fn test_error_invalid_let_binding() {
        let result = parse_expr_string("(let ((42 5)) x)");
        assert!(result.is_err());
    }
}