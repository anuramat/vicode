#[macro_export]
macro_rules! forward {
    ($($name:ident: $ty:ty = self$(.$field:ident)+;)+) => {
        paste::paste! {
            $(
                pub fn $name(&self) -> &$ty {
                    &self$(.$field)+
                }
                pub fn [<$name _mut>](&mut self) -> &mut $ty {
                    &mut self$(.$field)+
                }
            )+
        }
    };
}
