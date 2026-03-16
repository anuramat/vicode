use ratatui::prelude::*;

lazy_static::lazy_static! {
    pub static ref LOGO_VARIANTS: Logo = Logo::new();
    static ref STYLE: Style = Style::default().fg(Color::Red).bold();
}

pub struct LogoVariant {
    width: u16,
    pub height: u16,
    pub text: String,
}

pub struct Logo {
    variants: Vec<LogoVariant>,
    default: LogoVariant,
}

impl Widget for &Logo {
    fn render(
        self,
        area: Rect,
        buf: &mut Buffer,
    ) {
        buf.set_style(
            buf.area,
            // TODO centralize color scheme
            Style::default().fg(Color::from_u32(0x00333333)).not_bold(),
        );
        let logo = self.get_logo(area);
        let x_start = (area.width - logo.width) / 2;
        let y_start = (area.height - logo.height) / 2;
        for (y_rel, line) in logo.text.lines().enumerate() {
            let y_abs = y_start + y_rel as u16;
            if y_abs >= area.right() {
                break;
            }
            for (x_rel, ch) in line.chars().enumerate() {
                let x_abs = x_start + x_rel as u16;
                if x_abs >= area.right() {
                    break;
                }
                if !ch.is_whitespace() {
                    buf[(x_abs, y_abs)].set_char(ch).set_style(*STYLE);
                }
            }
        }
    }
}

impl LogoVariant {
    fn new(text: String) -> Self {
        let lines = text.lines().count() as u16;
        let max_line_width = text
            .lines()
            .map(|line| line.chars().count() as u16)
            .max()
            .unwrap_or(0);
        Self {
            width: max_line_width,
            height: lines,
            text,
        }
    }
}

impl Logo {
    pub fn get_logo(
        &self,
        area: Rect,
    ) -> &LogoVariant {
        let Rect {
            x: _,
            y: _,
            width,
            height,
        } = area;
        for logo in &self.variants {
            if logo.width <= width && logo.height <= height {
                return logo;
            }
        }
        &self.default
    }

    fn new() -> Self {
        let variants = vec![
            LogoVariant::new(LOGO_66X12.to_string()),
            LogoVariant::new(LOGO_23X12.to_string()),
        ];
        let default = LogoVariant::new("vc".to_string());
        Self { variants, default }
    }
}

const LOGO_66X12: &str = r#"
   _            .                             ..                  
  u            @88>                         dF                    
 88Nu.   u.    %8P                    u.   '88bu.                 
'88888.o888c    .          .    ...ue888b  '*88888bu        .u    
 ^8888  8888  .@88u   .udR88N   888R Y888r   ^"*8888N    ud8888.  
  8888  8888 ''888E` <888'888k  888R I888>  beWE "888L :888'8888. 
  8888  8888   888E  9888 'Y"   888R I888>  888E  888E d888 '88%" 
  8888  8888   888E  9888       888R I888>  888E  888E 8888.+"    
 .8888b.888P   888E  9888      u8888cJ888   888E  888F 8888L      
  ^Y8888*""    888&  ?8888u../  "*888*P"   .888N..888  '8888c. .+ 
    `Y"        R888"  "8888P'     'Y"       `"888*""    "88888%   
                ""      "P'                    ""         "YP'    
"#;

const LOGO_23X12: &str = r#"
   _                   
  u                    
 88Nu.   u.            
'88888.o888c       .   
 ^8888  8888  .udR88N  
  8888  8888 <888'888k 
  8888  8888 9888 'Y"  
  8888  8888 9888      
 .8888b.888P 9888      
  ^Y8888*""  ?8888u../ 
    `Y"       "8888P'  
                "P'    
"#;
