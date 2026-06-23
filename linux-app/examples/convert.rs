// Throwaway: convert a captured Poe HTML file to Markdown and print it.
fn main() {
    let path = std::env::args().nth(1).expect("usage: convert <file.html>");
    let html = std::fs::read_to_string(&path).unwrap();
    let c = ravenvault::html2md::html_to_markdown(&html, "Untitled");
    println!("TITLE: {}\nASSETS: {}\n----", c.title, c.asset_urls.len());
    println!("{}", c.markdown);
}
